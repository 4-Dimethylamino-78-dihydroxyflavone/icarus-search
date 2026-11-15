//! Fare intelligence: the two clocks of a ticket price.
//!
//! WHEN TO FLY - a year laid out like a contribution graph, each day
//! coloured by how the lowest fare seen for departing then ranks against
//! the rest of the year, with every month's cheapest and dearest day
//! picked out.
//! WHEN TO BUY - how the price of one particular trip has moved over the
//! weeks leading up to today, against its usual range.
//!
//! Everything here is written to be read cold, by someone who has never
//! seen a heatmap: headings are questions, every colour and symbol has a
//! key beside it, and each panel ends in a plain-language verdict.

use std::collections::HashSet;

use chrono::{Datelike, Duration, Local, Months, NaiveDate, Utc};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::{canvas::DotCanvas, pane_with, theme::Theme, title_style, trip_chip};
use crate::{app::App, farelog::FareMap, fmt};

const BOLD: Modifier = Modifier::BOLD;
const WEEKDAYS: [&str; 7] = ["MON", "TUE", "WED", "THU", "FRI", "SAT", "SUN"];
/// How far ahead the heatmap looks.
const SPAN_DAYS: i64 = 364;
const LOW_MARK: &str = "▼";
const HIGH_MARK: &str = "▲";
/// A day whose fare is estimated from its neighbours: nearly as solid as a
/// measured day, so the year still reads as one picture, but not quite.
const ESTIMATE: &str = "▓▓";
/// Width of the weekday names beside the heatmap.
const GUTTER: usize = 5;
/// Width of the reading guide shown beside the heatmap when there is room.
const GUIDE_WIDTH: usize = 44;
/// Bright white: the cursor, which must not look like any fare.
const CURSOR: Color = Color::Indexed(231);

pub fn draw(f: &mut Frame, area: Rect, app: &mut App) {
    let th = app.theme;
    let plain = title_style(true, th);
    let title = match app.form_query(app.form.depart) {
        Some(q) => {
            let nights = q.ret.map_or(String::new(), |ret| {
                format!(" {}D", (ret - q.depart).num_days())
            });
            Line::from(vec![
                Span::styled(format!(" FARE INTEL  {} ► {} ", q.origin, q.dest), plain),
                trip_chip(q.ret.is_some(), th),
                Span::styled(format!("{nights}  {} ", q.cabin.label()), plain),
            ])
        }
        None => Line::from(Span::styled(" FARE INTEL ", plain)),
    };
    let block = pane_with(title, true, th);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(map) = app.fare_map() else {
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                " NO ROUTE SET.",
                Style::new().fg(th.accent).add_modifier(BOLD),
            )),
            Line::from(Span::styled(
                " PRESS ESC, CHOOSE FROM AND TO, THEN COME BACK WITH I.",
                Style::new().fg(th.dim),
            )),
        ];
        f.render_widget(Paragraph::new(lines), inner);
        return;
    };
    let cursor = app.intel.as_ref().map_or(app.form.depart, |i| i.cursor);
    let [top, bottom] = Layout::vertical([Constraint::Length(13), Constraint::Min(0)]).areas(inner);
    let (grid, cell, start) = heatmap(f, top, app, &map, cursor);
    app.hit.heat_grid = grid;
    app.hit.heat_cell = cell;
    app.hit.heat_start = Some(start);

    let app = &*app;
    // The month table grows a column for each month's cheapest day, then
    // one for its dearest, as the window gets wide enough to spare them.
    let (months_width, extremes) = match bottom.width {
        150.. => (63, 2),
        122.. => (47, 1),
        _ => (31, 0),
    };
    let [months, lead, chart] = Layout::horizontal([
        Constraint::Length(months_width),
        Constraint::Length(33),
        Constraint::Min(30),
    ])
    .areas(bottom);
    by_month(f, months, app, &map, extremes);
    by_lead(f, lead, app, &map);
    booking_curve(f, chart, app, &map, cursor);
}

/// The first day of each calendar month the heatmap touches.
fn months_from(today: NaiveDate) -> impl Iterator<Item = NaiveDate> {
    let first = today.with_day(1).unwrap_or(today);
    (0..=12).filter_map(move |n| first.checked_add_months(Months::new(n)))
}

/// How to read this screen. `full` is the whole guide, for the `?` pop-up;
/// otherwise the short form that sits beside the heatmap.
pub fn guide_lines(th: Theme, full: bool) -> Vec<Line<'static>> {
    let head = |text: &'static str| {
        Line::from(Span::styled(
            text,
            Style::new().fg(th.accent).add_modifier(BOLD),
        ))
    };
    let body = |text: &'static str| Line::from(Span::styled(text, Style::new().fg(th.fg)));
    // A line that opens with a coloured sample of the thing it explains.
    let keyed = |sample: Span<'static>, text: &'static str| {
        Line::from(vec![
            Span::raw(" "),
            sample,
            Span::styled(text, Style::new().fg(th.fg)),
        ])
    };
    let cheap = Style::new().fg(th.heat(0.0)).add_modifier(BOLD);
    let dear = Style::new().fg(th.heat(1.0)).add_modifier(BOLD);
    if !full {
        return vec![
            head(" HOW TO READ THIS"),
            body(" EACH SQUARE IS A DAY YOU COULD LEAVE."),
            body(" ACROSS: WEEKS.  DOWN: MONDAY TO SUNDAY."),
            keyed(
                Span::styled("GREEN", cheap),
                "  CHEAPER THAN MOST OF THE YEAR",
            ),
            keyed(
                Span::styled("RED  ", dear),
                "  DEARER THAN MOST OF THE YEAR",
            ),
            keyed(
                Span::styled("▼ ▲  ", Style::new().fg(th.bright)),
                "  EACH MONTH'S BEST AND WORST DAY",
            ),
            body(" ARROWS OR A CLICK CHOOSE A DAY. THE CHART"),
            body(" BELOW THEN SHOWS HOW THAT TRIP'S PRICE HAS"),
            body(" MOVED, AND WHETHER NOW IS A GOOD TIME."),
            Line::from(Span::styled(
                " ENTER  FLY THAT DAY        ?  MORE",
                Style::new().fg(th.dim),
            )),
        ];
    }
    vec![
        head(" WHEN TO FLY  (THE GRID AT THE TOP)"),
        body("  EACH SQUARE IS ONE DAY YOU COULD DEPART. COLUMNS ARE WEEKS, STARTING"),
        body("  WITH THIS ONE; ROWS ARE MONDAY TO SUNDAY."),
        keyed(
            Span::styled(" GREEN", cheap),
            " DAYS ARE CHEAPER THAN MOST OF THE YEAR.",
        ),
        keyed(
            Span::styled(" RED  ", dear),
            " DAYS ARE DEARER THAN MOST OF THE YEAR.",
        ),
        body("  THE COLOUR IS A DAY'S RANK, NOT ITS PRICE: THE SCALE UNDER THE GRID"),
        body("  GIVES THE ACTUAL CHEAPEST, MIDDLE AND DEAREST FARES."),
        keyed(
            Span::styled(" ██", Style::new().fg(th.fg)),
            "  A FARE MEASURED BY A SEARCH.",
        ),
        keyed(
            Span::styled(format!(" {ESTIMATE}"), Style::new().fg(th.fg)),
            "  AN ESTIMATE, BETWEEN TWO MEASURED DAYS.",
        ),
        keyed(
            Span::styled(" ▼ ▲", Style::new().fg(th.bright)),
            " THE CHEAPEST AND DEAREST MEASURED DAY OF EACH MONTH.",
        ),
        Line::from(""),
        head(" WHICH MONTH  /  HOW MUCH NOTICE  (BOTTOM LEFT)"),
        body("  THE SAME FARES AVERAGED BY MONTH, AND BY HOW SOON YOU WOULD LEAVE."),
        body("  A LONGER BAR IS A DEARER MONTH. LOW AND PEAK TAG THE EXTREMES."),
        Line::from(""),
        head(" WHEN TO BUY  (THE CHART, BOTTOM RIGHT)"),
        body("  FOR THE DAY UNDER THE CURSOR: WHAT THAT EXACT TRIP HAS COST, DAY BY"),
        body("  DAY, UP TO TODAY. THE GREY BAND IS ITS USUAL PRICE RANGE."),
        keyed(
            Span::styled(" GREEN", Style::new().fg(th.good).add_modifier(BOLD)),
            " LINE UNDER THE BAND: A GOOD PRICE.",
        ),
        keyed(
            Span::styled(" AMBER", Style::new().fg(th.accent).add_modifier(BOLD)),
            " LINE INSIDE THE BAND: A NORMAL PRICE.",
        ),
        keyed(
            Span::styled(" RED  ", Style::new().fg(th.alert).add_modifier(BOLD)),
            " LINE OVER THE BAND: A BAD PRICE.",
        ),
        body("  THE VERDICT FOR TODAY IS SPELLED OUT ABOVE THE CHART."),
        Line::from(""),
        head(" KEYS"),
        body("  ARROWS OR CLICK  CHOOSE A DAY        ENTER  MAKE IT THE DEPARTURE DATE"),
        body("  V  SURVEY MORE DATES                 I OR ESC  BACK TO THE FLIGHTS"),
    ]
}

// ------------------------------------------------------------- when to fly

/// Returns the clickable day grid, how many cells wide each week is, and
/// the date of the grid's top-left cell.
fn heatmap(
    f: &mut Frame,
    area: Rect,
    app: &App,
    map: &FareMap,
    cursor: NaiveDate,
) -> (Rect, u16, NaiveDate) {
    let th = app.theme;
    let cur = &app.cfg.currency;
    let today = Local::now().date_naive();
    let last = today + Duration::days(SPAN_DAYS);
    // Columns are weeks, starting on the Monday of the current week.
    let monday = today - Duration::days(i64::from(today.weekday().num_days_from_monday()));
    let total_weeks = ((last - monday).num_days() / 7 + 1) as usize;
    // Wide squares if the whole year fits that way with the guide beside
    // it; else narrow squares with the guide; else just the squares.
    let width = usize::from(area.width);
    let fits = |cell: usize, guide: bool| {
        GUTTER + total_weeks * cell + if guide { GUIDE_WIDTH + 2 } else { 0 } <= width
    };
    let (cell, with_guide) = [(3, true), (2, true), (3, false)]
        .into_iter()
        .find(|(cell, guide)| fits(*cell, *guide))
        .unwrap_or((2, false));
    let weeks = (width.saturating_sub(GUTTER + 1) / cell)
        .min(total_weeks)
        .max(1);
    // Scroll horizontally if the cursor's week is off the right-hand edge.
    let cursor_week = ((cursor - monday).num_days().max(0) / 7) as usize;
    let first_week = (cursor_week + 1)
        .saturating_sub(weeks)
        .min(total_weeks - weeks);
    let start = monday + Duration::days(first_week as i64 * 7);

    // Every month's cheapest and dearest measured day.
    let (mut lows, mut highs) = (HashSet::new(), HashSet::new());
    for first in months_from(today) {
        if let Some(((low, _), (high, _))) = map.month_extremes(first, today) {
            lows.insert(low);
            highs.insert(high);
        }
    }

    let measured = map.known.keys().filter(|d| **d >= today).count();
    let mut lines = vec![Line::from(vec![
        Span::styled(
            " WHEN TO FLY",
            Style::new().fg(th.accent).add_modifier(BOLD),
        ),
        Span::styled(
            "  WHICH DAYS ARE CHEAP TO LEAVE ON?",
            Style::new().fg(th.fg),
        ),
        Span::styled(
            format!(
                "   {measured} DAY{} MEASURED SO FAR",
                if measured == 1 { "" } else { "S" }
            ),
            Style::new().fg(th.dim),
        ),
    ])];

    // Month names sit above the week in which each month begins.
    let mut months = vec![' '; GUTTER + weeks * cell];
    for week in 0..weeks {
        let week_start = start + Duration::days(week as i64 * 7);
        let first_of_month = (0..7)
            .map(|d| week_start + Duration::days(d))
            .find(|d| d.day() == 1);
        if let Some(day) = first_of_month.or_else(|| (week == 0).then_some(week_start)) {
            for (i, ch) in day
                .format("%b")
                .to_string()
                .to_uppercase()
                .chars()
                .enumerate()
            {
                if let Some(slot) = months.get_mut(GUTTER + week * cell + i) {
                    *slot = ch;
                }
            }
        }
    }
    lines.push(Line::from(Span::styled(
        months.into_iter().collect::<String>(),
        Style::new().fg(th.dim),
    )));

    let gap = " ".repeat(cell - 2);
    for (weekday, name) in WEEKDAYS.iter().enumerate() {
        let mut spans = vec![Span::styled(format!(" {name} "), Style::new().fg(th.dim))];
        for week in 0..weeks {
            let date = start + Duration::days((week * 7 + weekday) as i64);
            let (low, high) = (lows.contains(&date), highs.contains(&date));
            let mark = if low {
                LOW_MARK
            } else if high {
                HIGH_MARK
            } else {
                "●"
            };
            // A solid tile for a measured fare, a faded one for an
            // estimate; a month's best and worst day keep their tile and
            // carry a mark on it; the cursor is a white tile like no fare.
            let tile = if date < today || date > last {
                Span::raw("  ")
            } else if date == cursor {
                Span::styled(
                    format!("{mark} "),
                    Style::new().fg(th.bg).bg(CURSOR).add_modifier(BOLD),
                )
            } else if low || high {
                let color = th.heat(if low { 0.0 } else { 1.0 });
                Span::styled(
                    format!("{mark} "),
                    Style::new().fg(th.bg).bg(color).add_modifier(BOLD),
                )
            } else {
                match map.at(date) {
                    Some((price, true)) => {
                        Span::styled("██", Style::new().fg(th.heat(map.heat(price))))
                    }
                    Some((price, false)) => {
                        Span::styled(ESTIMATE, Style::new().fg(th.heat(map.heat(price))))
                    }
                    None => Span::styled("· ", Style::new().fg(th.faint)),
                }
            };
            spans.push(tile);
            if cell > 2 {
                spans.push(Span::raw(gap.clone()));
            }
        }
        lines.push(Line::from(spans));
    }
    let grid = Rect::new(area.x + GUTTER as u16, area.y + 2, (weeks * cell) as u16, 7);

    // The colour scale, with real fares at its ends and middle.
    let key = |text: &'static str| Span::styled(text, Style::new().fg(th.dim));
    if map.is_empty() {
        lines.push(Line::from(""));
    } else {
        let mut scale = vec![
            key(" CHEAPEST "),
            Span::styled(
                fmt::money(map.lo, cur),
                Style::new().fg(th.heat(0.0)).add_modifier(BOLD),
            ),
            Span::raw(" "),
        ];
        for step in 0..9 {
            scale.push(Span::styled(
                "██",
                Style::new().fg(th.heat(f64::from(step) / 8.0)),
            ));
        }
        scale.push(Span::raw(" "));
        scale.push(Span::styled(
            fmt::money(map.hi, cur),
            Style::new().fg(th.heat(1.0)).add_modifier(BOLD),
        ));
        scale.push(key(" DEAREST"));
        if let Some(median) = map.median() {
            scale.push(key("    HALF THE DAYS ARE UNDER "));
            scale.push(Span::styled(
                fmt::money(median, cur),
                Style::new().fg(th.heat(0.5)).add_modifier(BOLD),
            ));
        }
        lines.push(Line::from(scale));
    }
    // What each kind of square means, drawn as it appears in the grid.
    let on = |heat: f64| Style::new().fg(th.bg).bg(th.heat(heat)).add_modifier(BOLD);
    lines.push(Line::from(vec![
        Span::raw(" "),
        Span::styled("██", Style::new().fg(th.fg)),
        key(" MEASURED  "),
        Span::styled(ESTIMATE, Style::new().fg(th.fg)),
        key(" ESTIMATED  "),
        Span::styled("· ", Style::new().fg(th.fg)),
        key("NO DATA  "),
        Span::styled(format!("{LOW_MARK} "), on(0.0)),
        key(" MONTH'S CHEAPEST DAY  "),
        Span::styled(format!("{HIGH_MARK} "), on(1.0)),
        key(" MONTH'S DEAREST DAY  "),
        Span::styled("● ", Style::new().fg(th.bg).bg(CURSOR).add_modifier(BOLD)),
        key(" YOU ARE HERE"),
    ]));

    // What the cursor is on, and how it ranks.
    let ahead = (cursor - today).num_days();
    let mut info = vec![
        Span::styled(
            format!(" ► {}", fmt::day(cursor)),
            Style::new().fg(th.bright).add_modifier(BOLD),
        ),
        Span::styled(
            format!(
                "  LEAVES IN {ahead} DAY{}  ",
                if ahead == 1 { "" } else { "S" }
            ),
            Style::new().fg(th.dim),
        ),
    ];
    let ranked = |price: f64| {
        let heat = map.heat(price);
        let beaten = ((1.0 - heat) * 100.0).round() as u32;
        let text = match beaten {
            100 => "  THE CHEAPEST DAY MEASURED".to_string(),
            0 => "  THE DEAREST DAY MEASURED".to_string(),
            n => format!("  CHEAPER THAN {n}% OF THE YEAR"),
        };
        Span::styled(text, Style::new().fg(th.heat(heat)))
    };
    match (map.at(cursor), map.known.get(&cursor)) {
        (Some((price, _)), Some(sighting)) => {
            let age_min = ((Utc::now().timestamp() - sighting.seen).max(0) / 60) as u32;
            info.push(Span::styled(
                fmt::money(price, cur),
                Style::new().fg(th.heat(map.heat(price))).add_modifier(BOLD),
            ));
            info.push(ranked(price));
            let month = cursor.format("%B").to_string().to_uppercase();
            if lows.contains(&cursor) {
                info.push(Span::styled(
                    format!("  {LOW_MARK} CHEAPEST IN {month}"),
                    Style::new().fg(th.heat(0.0)).add_modifier(BOLD),
                ));
            } else if highs.contains(&cursor) {
                info.push(Span::styled(
                    format!("  {HIGH_MARK} DEAREST IN {month}"),
                    Style::new().fg(th.heat(1.0)).add_modifier(BOLD),
                ));
            }
            let when = if age_min == 0 {
                "JUST NOW".to_string()
            } else {
                format!("{} AGO", age(age_min))
            };
            info.push(Span::styled(
                format!("  MEASURED {when}"),
                Style::new().fg(th.dim),
            ));
        }
        (Some((price, _)), None) => {
            info.push(Span::styled(
                format!("ABOUT {}", fmt::money(price, cur)),
                Style::new().fg(th.heat(map.heat(price))),
            ));
            info.push(ranked(price));
            info.push(Span::styled(
                "  AN ESTIMATE FROM THE DAYS EITHER SIDE",
                Style::new().fg(th.dim),
            ));
        }
        _ if app.survey_left > 0 => {
            info.push(Span::styled(
                "SURVEYING THE YEAR...",
                Style::new().fg(th.accent).add_modifier(BOLD),
            ));
        }
        _ if map.is_empty() => {
            info.push(Span::styled(
                "NO FARES LOGGED YET - PRESS V TO SURVEY",
                Style::new().fg(th.accent).add_modifier(BOLD),
            ));
        }
        _ => info.push(Span::styled(
            "NO DATA - PRESS V TO SURVEY, OR ENTER THEN S TO SCAN IT",
            Style::new().fg(th.dim),
        )),
    }
    lines.push(Line::from(info));
    f.render_widget(Paragraph::new(lines), area);

    if with_guide {
        let x = area.x + (GUTTER + weeks * cell + 2) as u16;
        let guide = Rect::new(
            x,
            area.y + 1,
            area.right().saturating_sub(x),
            area.height.saturating_sub(3),
        );
        f.render_widget(Paragraph::new(guide_lines(th, false)), guide);
    }
    (grid, cell as u16, start)
}

/// "42M", "3H", "5D"
fn age(minutes: u32) -> String {
    match minutes {
        0..60 => format!("{minutes}M"),
        60..1440 => format!("{}H", minutes / 60),
        _ => format!("{}D", minutes / 1440),
    }
}

/// One labelled bar per row, coloured and tagged relative to the others.
fn bar_rows(
    app: &App,
    rows: &[(String, Option<f64>, String)],
    label_width: usize,
) -> Vec<Line<'static>> {
    let th = app.theme;
    let values: Vec<f64> = rows.iter().filter_map(|r| r.1).collect();
    let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    rows.iter()
        .map(|(label, value, note)| {
            let mut spans = vec![Span::styled(
                format!(" {label:<label_width$} "),
                Style::new().fg(th.fg),
            )];
            match value {
                Some(v) => {
                    let t = if hi - lo > 1e-9 {
                        (v - lo) / (hi - lo)
                    } else {
                        0.0
                    };
                    let filled = 1 + (t * 7.0).round() as usize;
                    let color = th.heat(t);
                    let tag = if values.len() < 2 {
                        note.as_str()
                    } else if *v <= lo {
                        "LOW"
                    } else if *v >= hi {
                        "PEAK"
                    } else {
                        note.as_str()
                    };
                    spans.push(Span::styled("█".repeat(filled), Style::new().fg(color)));
                    spans.push(Span::styled(
                        "░".repeat(8 - filled),
                        Style::new().fg(th.faint),
                    ));
                    spans.push(Span::styled(
                        format!(" {:>7}", fmt::money(*v, &app.cfg.currency)),
                        Style::new().fg(color).add_modifier(BOLD),
                    ));
                    spans.push(Span::styled(format!(" {tag:<4}"), Style::new().fg(color)));
                }
                None => spans.push(Span::styled(
                    "········       -     ",
                    Style::new().fg(th.faint),
                )),
            }
            Line::from(spans)
        })
        .collect()
}

/// `extremes`: 0 shows averages only, 1 adds each month's cheapest day,
/// 2 its dearest as well.
fn by_month(f: &mut Frame, area: Rect, app: &App, map: &FareMap, extremes: usize) {
    let th = app.theme;
    let today = Local::now().date_naive();
    let months = map.by_month(today, 12);
    let rows: Vec<(String, Option<f64>, String)> = months
        .iter()
        .map(|(first, average, measured, _)| {
            // An average resting on one or two real samples is flagged.
            let note = if average.is_some() && *measured < 3 {
                "~"
            } else {
                ""
            };
            (
                first.format("%b %y").to_string().to_uppercase(),
                *average,
                note.to_string(),
            )
        })
        .collect();
    let mut heading = vec![Span::styled(
        " MONTH   AVERAGE FARE         ",
        Style::new().fg(th.dim),
    )];
    if extremes >= 1 {
        heading.push(Span::styled(" ITS CHEAPEST DAY", Style::new().fg(th.dim)));
    }
    if extremes >= 2 {
        heading.push(Span::styled(" ITS DEAREST DAY", Style::new().fg(th.dim)));
    }
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                " WHICH MONTH?",
                Style::new().fg(th.accent).add_modifier(BOLD),
            ),
            Span::styled("  LONGER BAR = DEARER", Style::new().fg(th.fg)),
        ]),
        Line::from(heading),
    ];
    let rough = rows.iter().any(|r| r.2 == "~");
    let mut bars = bar_rows(app, &rows, 6);
    for (line, (first, ..)) in bars.iter_mut().zip(&months) {
        let Some((low, high)) = map.month_extremes(*first, today) else {
            continue;
        };
        let day = |(date, price): (NaiveDate, f64), mark: &str| {
            format!(
                " {mark}{} {:<7}",
                date.format("%a %d").to_string().to_uppercase(),
                fmt::money(price, &app.cfg.currency)
            )
        };
        if extremes >= 1 {
            line.spans.push(Span::styled(
                day(low, LOW_MARK),
                Style::new().fg(th.heat(0.0)),
            ));
        }
        if extremes >= 2 {
            line.spans.push(Span::styled(
                day(high, HIGH_MARK),
                Style::new().fg(th.heat(1.0)),
            ));
        }
    }
    lines.extend(bars);
    if rough {
        lines.push(Line::from(Span::styled(
            " ~ ROUGH: FEWER THAN 3 DAYS MEASURED",
            Style::new().fg(th.dim),
        )));
    }
    f.render_widget(Paragraph::new(lines), area);
}

fn by_lead(f: &mut Frame, area: Rect, app: &App, map: &FareMap) {
    let th = app.theme;
    let today = Local::now().date_naive();
    let bands = map.by_lead(today);
    let rows: Vec<(String, Option<f64>, String)> = bands
        .iter()
        .map(|(label, average, _)| (label.to_string(), *average, String::new()))
        .collect();
    let mut lines = vec![
        Line::from(vec![Span::styled(
            " HOW MUCH NOTICE?",
            Style::new().fg(th.accent).add_modifier(BOLD),
        )]),
        Line::from(Span::styled(
            " LEAVING   AVERAGE FARE",
            Style::new().fg(th.dim),
        )),
    ];
    lines.extend(bar_rows(app, &rows, 7));
    // Say what the bars say, in words.
    let priced: Vec<(&str, f64)> = bands
        .iter()
        .filter_map(|(label, average, _)| Some((*label, (*average)?)))
        .collect();
    let cheapest = priced.iter().copied().min_by(|a, b| a.1.total_cmp(&b.1));
    let dearest = priced.iter().copied().max_by(|a, b| a.1.total_cmp(&b.1));
    if let (Some((cheap, low)), Some((dear, high))) = (cheapest, dearest) {
        if priced.len() >= 2 && high > low {
            let more = ((high / low - 1.0) * 100.0).round();
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled(" CHEAPEST LEAVING IN ", Style::new().fg(th.fg)),
                Span::styled(
                    cheap.to_string(),
                    Style::new().fg(th.heat(0.0)).add_modifier(BOLD),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled(format!(" {dear} COSTS "), Style::new().fg(th.fg)),
                Span::styled(
                    format!("{more:.0}% MORE"),
                    Style::new().fg(th.heat(1.0)).add_modifier(BOLD),
                ),
            ]));
        }
    }
    lines.push(Line::from(""));
    // Honest caveat: with one snapshot in time, these two effects overlap.
    lines.push(Line::from(Span::styled(
        " TODAY'S PRICES ONLY, SO SEASON",
        Style::new().fg(th.dim),
    )));
    lines.push(Line::from(Span::styled(
        " AND NOTICE ARE MIXED TOGETHER.",
        Style::new().fg(th.dim),
    )));
    lines.push(Line::from(Span::styled(
        " THE CHART SHOWS TIMING ALONE.",
        Style::new().fg(th.dim),
    )));
    f.render_widget(Paragraph::new(lines), area);
}

// -------------------------------------------------------------- when to buy

/// Price of the cursor's trip over the days leading up to now.
fn booking_curve(f: &mut Frame, area: Rect, app: &App, map: &FareMap, cursor: NaiveDate) {
    let th = app.theme;
    let cur = &app.cfg.currency;
    let mut lines = vec![Line::from(vec![
        Span::styled(
            " WHEN TO BUY",
            Style::new().fg(th.accent).add_modifier(BOLD),
        ),
        Span::styled(
            format!(
                "  WHAT LEAVING ON {} HAS COST, DAY BY DAY",
                fmt::day_short(cursor)
            ),
            Style::new().fg(th.fg),
        ),
    ])];
    let insights = app
        .route_key()
        .and_then(|key| app.insights.get(&(key, cursor)));
    // Prefer the provider's daily history; fall back to our own sightings.
    let own: Vec<f64> = app
        .route_key()
        .map(|key| {
            app.fares
                .history(&key, cursor)
                .into_iter()
                .map(|(_, price)| price)
                .collect()
        })
        .unwrap_or_default();
    let (series, source) = match insights {
        Some(i) if i.history.len() >= 2 => {
            (i.history.clone(), format!("{} DAYS AGO", i.history.len()))
        }
        _ => (own, "FIRST SIGHTING".to_string()),
    };

    if series.len() < 2 {
        let message = if map.known.contains_key(&cursor) {
            " ONE PRICE SO FAR. PRESS ENTER THEN S TO SCAN THIS DAY FOR ITS HISTORY, OR LOOK AGAIN ANOTHER DAY."
        } else {
            " NO HISTORY FOR THIS DAY YET. PRESS ENTER THEN S TO SCAN IT."
        };
        lines.push(Line::from(Span::styled(message, Style::new().fg(th.dim))));
        f.render_widget(
            Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false }),
            area,
        );
        return;
    }

    let typical = insights.and_then(|i| i.typical);
    // Where a fare stands against the usual range decides its colour, here
    // and in the verdict: under it, inside it, or over it.
    let zone = |price: f64| match typical {
        Some((a, _)) if price < a => th.good,
        Some((_, b)) if price > b => th.alert,
        _ => th.accent,
    };
    let lo = series.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = series.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let now = series[series.len() - 1];

    // The verdict comes first: it is the answer the chart is evidence for.
    let recent = series[series.len().saturating_sub(15)];
    let trend = (now - recent) / recent * 100.0;
    let (movement, movement_color) = if trend <= -2.0 {
        (format!("DOWN {:.0}% IN 2 WEEKS", -trend), th.good)
    } else if trend >= 2.0 {
        (format!("UP {trend:.0}% IN 2 WEEKS"), th.alert)
    } else {
        ("STEADY FOR 2 WEEKS".to_string(), th.fg)
    };
    let standing = match typical {
        Some((a, _)) if now < a => "BELOW USUAL: A GOOD PRICE",
        Some((_, b)) if now > b => "ABOVE USUAL: A POOR PRICE",
        Some(_) => "WITHIN USUAL: A NORMAL PRICE",
        None if now <= lo => "THE LOWEST IT HAS BEEN",
        None if now >= hi => "THE HIGHEST IT HAS BEEN",
        None => "BETWEEN ITS LOW AND ITS HIGH",
    };
    lines.push(Line::from(vec![
        Span::styled(" TODAY ", Style::new().fg(th.dim)),
        Span::styled(
            fmt::money(now, cur),
            Style::new().fg(zone(now)).add_modifier(BOLD),
        ),
        Span::styled(
            format!("  {standing}"),
            Style::new().fg(zone(now)).add_modifier(BOLD),
        ),
        Span::styled("   ", Style::new()),
        Span::styled(movement, Style::new().fg(movement_color).add_modifier(BOLD)),
    ]));

    // The chart's range also covers the typical band, so the band is visible.
    let (floor, ceiling) = match typical {
        Some((a, b)) => (lo.min(a), hi.max(b)),
        None => (lo, hi),
    };
    let pad = ((ceiling - floor) * 0.08).max(1.0);
    let (floor, ceiling) = (floor - pad, ceiling + pad);

    let foot_rows = 3; // axis captions, the figures, the colour key
    let chart = Rect::new(
        area.x + 9,
        area.y + 3,
        area.width.saturating_sub(11),
        area.height.saturating_sub(3 + foot_rows),
    );
    if chart.width >= 8 && chart.height >= 3 {
        let mut cv = DotCanvas::new(chart.width, chart.height, app.marker);
        let (w, h) = (cv.width(), cv.height());
        let y_of = |price: f64| (h - 1.0) * (1.0 - (price - floor) / (ceiling - floor));
        let x_of = |i: usize| (w - 1.0) * i as f64 / (series.len() - 1) as f64;
        if let Some((a, b)) = typical {
            // Dotted rails mark the exact edges of the usual range.
            for edge in [a, b] {
                let mut x = 0.0;
                while x < w {
                    cv.plot(x, y_of(edge), th.dim, 1);
                    x += 3.0;
                }
            }
        }
        for i in 1..series.len() {
            let color = zone((series[i - 1] + series[i]) / 2.0);
            let (x0, y0, x1, y1) = (x_of(i - 1), y_of(series[i - 1]), x_of(i), y_of(series[i]));
            // Two dots thick: a single row of braille dots is a faint thread.
            cv.line(x0, y0, x1, y1, color, 5);
            cv.line(x0, y0 + 1.0, x1, y1 + 1.0, color, 5);
        }
        // Today: a bright blob on the end of the line.
        for dx in -2..=0 {
            for dy in -1..=2 {
                cv.plot(
                    w - 1.0 + f64::from(dx),
                    y_of(now) + f64::from(dy),
                    CURSOR,
                    9,
                );
            }
        }
        let buf = f.buffer_mut();
        let axis = Style::new().fg(th.dim);
        let row_of = |price: f64| chart.y + ((y_of(price) / 4.0) as u16).min(chart.height - 1);
        if let Some((a, b)) = typical {
            // The usual range is a shaded band right across the chart: the
            // line is inside it, below it or above it at a glance.
            let (top, bottom) = (row_of(b), row_of(a));
            for row in top..=bottom {
                buf.set_style(
                    Rect::new(chart.x, row, chart.width, 1),
                    Style::new().bg(th.band()),
                );
            }
            // Captioned one row in from an edge, clear of the dotted rail
            // that runs along the edge itself - and from whichever edge the
            // price line is not passing under the caption.
            let caption = " USUAL PRICE RANGE ";
            if usize::from(chart.width) > caption.len() + 4 && bottom >= top + 2 {
                let under_caption = |row: u16| {
                    series.iter().enumerate().any(|(i, price)| {
                        let col = (x_of(i) / 2.0) as usize;
                        col <= caption.len() + 2 && row_of(*price).abs_diff(row) <= 1
                    })
                };
                let row = if under_caption(top + 1) && !under_caption(bottom - 1) {
                    bottom - 1
                } else {
                    top + 1
                };
                let style = Style::new().fg(th.fg).bg(th.band());
                buf.set_stringn(chart.x + 1, row, caption, caption.len(), style);
            }
            // Its two edges are priced on the axis.
            for (edge, row) in [(b, top), (a, bottom)] {
                if row > chart.y && row + 1 < chart.bottom() {
                    buf.set_stringn(
                        area.x + 1,
                        row,
                        fmt::pad_left(&fmt::money(edge, cur), 7, ""),
                        7,
                        Style::new().fg(th.fg),
                    );
                }
            }
        }
        cv.render(chart, buf);
        buf.set_stringn(
            area.x + 1,
            chart.y,
            fmt::pad_left(&fmt::money(ceiling, cur), 7, ""),
            7,
            axis,
        );
        buf.set_stringn(
            area.x + 1,
            chart.bottom() - 1,
            fmt::pad_left(&fmt::money(floor, cur), 7, ""),
            7,
            axis,
        );
        // Name the end of the line, just above it (or below, at the top).
        let tag = format!("TODAY {}", fmt::money(now, cur));
        let tag_width = tag.chars().count() as u16;
        if chart.width > tag_width + 6 {
            let at = row_of(now);
            let row = if at > chart.y + 1 {
                at - 2
            } else {
                (at + 2).min(chart.bottom() - 1)
            };
            let style = Style::new().fg(th.bg).bg(CURSOR).add_modifier(BOLD);
            buf.set_stringn(
                chart.right() - tag_width - 3,
                row,
                &tag,
                usize::from(tag_width),
                style,
            );
        }
        let foot = chart.bottom();
        if foot < area.bottom() {
            buf.set_stringn(chart.x, foot, &source, usize::from(chart.width), axis);
            let label = "TODAY";
            if chart.width > 30 {
                buf.set_stringn(
                    chart.right() - label.len() as u16,
                    foot,
                    label,
                    label.len(),
                    axis,
                );
            }
        }
    }

    // Under the chart: the figures, then the key to its colours.
    let mut summary = vec![Line::from(vec![
        Span::styled(" LOW ", Style::new().fg(th.dim)),
        Span::styled(
            fmt::money(lo, cur),
            Style::new().fg(th.good).add_modifier(BOLD),
        ),
        Span::styled("   HIGH ", Style::new().fg(th.dim)),
        Span::styled(
            fmt::money(hi, cur),
            Style::new().fg(th.alert).add_modifier(BOLD),
        ),
        Span::styled(
            match typical {
                Some((a, b)) => format!(
                    "   USUALLY {} TO {}",
                    fmt::money(a, cur),
                    fmt::money(b, cur)
                ),
                None => String::new(),
            },
            Style::new().fg(th.fg),
        ),
    ])];
    if typical.is_some() {
        summary.push(Line::from(vec![
            Span::styled(" LINE: ", Style::new().fg(th.dim)),
            Span::styled("GREEN", Style::new().fg(th.good).add_modifier(BOLD)),
            Span::styled(" BELOW USUAL   ", Style::new().fg(th.dim)),
            Span::styled("AMBER", Style::new().fg(th.accent).add_modifier(BOLD)),
            Span::styled(" INSIDE ", Style::new().fg(th.dim)),
            Span::styled("   ", Style::new().bg(th.band())),
            Span::styled("   ", Style::new().fg(th.dim)),
            Span::styled("RED", Style::new().fg(th.alert).add_modifier(BOLD)),
            Span::styled(" ABOVE", Style::new().fg(th.dim)),
        ]));
    }
    f.render_widget(
        Paragraph::new(lines),
        Rect {
            height: 2.min(area.height),
            ..area
        },
    );
    let stats_y = area.bottom().saturating_sub(foot_rows - 1);
    if stats_y >= area.y + 3 {
        f.render_widget(
            Paragraph::new(summary),
            Rect::new(area.x, stats_y, area.width, area.bottom() - stats_y),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::PHOSPHOR;

    #[test]
    fn ages_read_naturally() {
        assert_eq!(age(0), "0M");
        assert_eq!(age(59), "59M");
        assert_eq!(age(60), "1H");
        assert_eq!(age(1439), "23H");
        assert_eq!(age(1440 * 3), "3D");
    }

    #[test]
    fn the_heatmap_covers_thirteen_calendar_months() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 15).unwrap();
        let months: Vec<NaiveDate> = months_from(today).collect();
        assert_eq!(
            months.len(),
            13,
            "364 days from mid-November reaches into next November"
        );
        assert_eq!(months[0], NaiveDate::from_ymd_opt(2026, 11, 1).unwrap());
        assert_eq!(months[12], NaiveDate::from_ymd_opt(2027, 11, 1).unwrap());
    }

    #[test]
    fn the_short_guide_fits_beside_the_heatmap() {
        let short = guide_lines(PHOSPHOR, false);
        assert!(short.len() <= 10, "the heatmap block is that tall");
        assert!(
            short.iter().all(|line| line.width() <= GUIDE_WIDTH),
            "a line is too wide for its column"
        );
        // The pop-up's lines fit its 76 columns.
        assert!(
            guide_lines(PHOSPHOR, true)
                .iter()
                .all(|line| line.width() <= 76)
        );
    }
}
