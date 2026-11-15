//! Modal overlays: airport picker, calendar, text entry, dossier, survey
//! confirmation and help.

use chrono::{Datelike, Duration, Local, NaiveDate};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Wrap},
};

use super::{intel, pane, panels};
use crate::{
    airports::Airport,
    app::{App, Endpoint, HORIZONS, KeyState, Popup, Setting, TextKind, YEAR},
    config::KeySource,
    currency, fmt,
};

const BOLD: Modifier = Modifier::BOLD;

/// Regions a popup wants clicks routed to.
#[derive(Default)]
struct Regions {
    popup: Rect,
    list: Rect,
    /// Index of the first row shown in `list`.
    list_offset: usize,
    grid: Rect,
    /// Dossier scroll after clamping to the content height.
    scroll: Option<u16>,
}

pub fn draw(f: &mut Frame, area: Rect, app: &mut App) {
    if app.popup.is_some() {
        // Everything behind a pop-up steps back into the dark, so that the
        // half-covered panels around it do not compete with it.
        f.buffer_mut()
            .set_style(area, Style::new().fg(app.theme.faint).bg(app.theme.bg));
    }
    let regions = match &app.popup {
        None => return,
        Some(Popup::Airport {
            target,
            input,
            hits,
            sel,
        }) => airport(f, area, app, *target, input, hits, *sel),
        Some(Popup::Calendar { ret, cursor }) => calendar(f, area, app, *ret, *cursor),
        Some(Popup::Text { kind, input, error }) => {
            text(f, area, app, *kind, input, error.as_deref())
        }
        Some(Popup::Dossier { scroll }) => dossier(f, area, app, *scroll),
        Some(Popup::Survey { horizon }) => survey(f, area, app, *horizon),
        Some(Popup::Currency { input, sel }) => currency_picker(f, area, app, input, *sel),
        Some(Popup::Settings { sel }) => settings(f, area, app, *sel),
        Some(Popup::Help) => help(f, area, app),
        Some(Popup::Guide) => guide(f, area, app),
    };
    app.hit.popup = regions.popup;
    app.hit.popup_list = regions.list;
    app.hit.popup_list_offset = regions.list_offset;
    app.hit.cal_grid = regions.grid;
    if let (Some(clamped), Some(Popup::Dossier { scroll })) = (regions.scroll, &mut app.popup) {
        *scroll = clamped;
    }
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let (w, h) = (width.min(area.width), height.min(area.height));
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

/// Clear a centred box, draw its frame, and return (outer, inner).
fn open(
    f: &mut Frame,
    area: Rect,
    app: &App,
    title: &str,
    width: u16,
    height: u16,
) -> (Rect, Rect) {
    let outer = centered(area, width, height);
    let block = pane(title, true, app.theme);
    let inner = block.inner(outer);
    f.render_widget(Clear, outer);
    f.render_widget(block, outer);
    (outer, inner)
}

/// A block cursor that blinks on the animation clock.
fn caret(app: &App) -> Span<'static> {
    let on = app.frame(2.5, 2) == 0;
    Span::styled(
        if on { "█" } else { " " },
        Style::new().fg(app.theme.bright),
    )
}

// ------------------------------------------------------------------ airport

fn airport(
    f: &mut Frame,
    area: Rect,
    app: &App,
    target: Endpoint,
    input: &str,
    hits: &[&'static Airport],
    sel: usize,
) -> Regions {
    let th = app.theme;
    let title = if target == Endpoint::Origin {
        "SELECT ORIGIN"
    } else {
        "SELECT DESTINATION"
    };
    let (outer, inner) = open(f, area, app, title, 70, 18);
    let width = usize::from(inner.width);
    let mut lines = vec![
        Line::from(vec![
            Span::styled(" ► ", Style::new().fg(th.accent).add_modifier(BOLD)),
            Span::styled(
                input.to_uppercase(),
                Style::new().fg(th.bright).add_modifier(BOLD),
            ),
            caret(app),
        ]),
        Line::from(Span::styled("─".repeat(width), Style::new().fg(th.faint))),
    ];
    if input.trim().is_empty() && !hits.is_empty() {
        // Opened by a click on the globe that covered several airports.
        lines.push(Line::from(Span::styled(
            " SEVERAL AIRPORTS SHARE THAT SPOT. CHOOSE ONE, OR TYPE TO SEARCH.",
            Style::new().fg(th.accent),
        )));
    } else if input.trim().is_empty() {
        lines.push(Line::from(Span::styled(
            " TYPE A CITY, AN AIRPORT NAME OR A 3-LETTER CODE",
            Style::new().fg(th.dim),
        )));
        lines.push(Line::from(Span::styled(
            " E.G.  TOKYO   HEATHROW   JFK",
            Style::new().fg(th.dim),
        )));
    } else if hits.is_empty() {
        lines.push(Line::from(Span::styled(
            " NO AIRPORT MATCHES THAT",
            Style::new().fg(th.accent),
        )));
    }
    // The clickable list starts wherever the lines above ended.
    let above = lines.len() as u16;
    let list = Rect::new(
        inner.x,
        inner.y + above,
        inner.width,
        inner.height.saturating_sub(above),
    );
    // Scroll just far enough to keep the selection in view.
    let visible = usize::from(list.height).max(1);
    let offset = (sel + 1).saturating_sub(visible);
    for (i, airport) in hits.iter().enumerate().skip(offset).take(visible) {
        let selected = i == sel;
        let base = if selected {
            Style::new().bg(th.sel_bg).fg(th.bright).add_modifier(BOLD)
        } else {
            Style::new().fg(th.fg)
        };
        // "TOKYO HANEDA INTL", but "PARIS - CHARLES DE GAULLE INTL".
        let (city, name) = (airport.city.to_uppercase(), airport.name.to_uppercase());
        let name = if name.contains(&city) {
            name
        } else {
            format!("{city} - {name}")
        };
        let size = ["MAJOR", "", "SMALL"][usize::from(airport.size.min(2))];
        let row = format!(
            " {}  {}  {}  {:<5} ",
            airport.iata,
            fmt::pad(&name, width.saturating_sub(18), app.glyphs.ellipsis),
            airport.country,
            size
        );
        lines.push(Line::from(vec![
            Span::styled(if selected { "►" } else { " " }, base.fg(th.accent)),
            Span::styled(fmt::pad(&row, width.saturating_sub(1), ""), base),
        ]));
    }
    f.render_widget(Paragraph::new(lines), inner);
    let shown = hits.len().saturating_sub(offset).min(visible) as u16;
    Regions {
        popup: outer,
        list: Rect {
            height: shown,
            ..list
        },
        list_offset: offset,
        ..Regions::default()
    }
}

// ----------------------------------------------------------------- currency

fn currency_picker(f: &mut Frame, area: Rect, app: &App, input: &str, sel: usize) -> Regions {
    let th = app.theme;
    let (outer, inner) = open(f, area, app, "CURRENCY", 52, 20);
    let width = usize::from(inner.width);
    let hits = currency::search(input);
    let mut lines = vec![
        Line::from(vec![
            Span::styled(" ► ", Style::new().fg(th.accent).add_modifier(BOLD)),
            Span::styled(
                input.to_uppercase(),
                Style::new().fg(th.bright).add_modifier(BOLD),
            ),
            caret(app),
        ]),
        Line::from(Span::styled("─".repeat(width), Style::new().fg(th.faint))),
    ];
    if input.trim().is_empty() {
        lines.push(Line::from(Span::styled(
            " CHOOSE ONE, OR TYPE A NAME OR ANY 3-LETTER CODE",
            Style::new().fg(th.dim),
        )));
    } else if hits.is_empty() {
        // Not in the list: any well-formed code can still be asked for.
        lines.push(match currency::normalize(input) {
            Some(code) => Line::from(vec![
                Span::styled(" NOT IN THE LIST. ", Style::new().fg(th.dim)),
                Span::styled(
                    format!("ENTER TO USE {code} AS TYPED"),
                    Style::new().fg(th.accent).add_modifier(BOLD),
                ),
            ]),
            None => Line::from(Span::styled(
                " NO MATCH. A CURRENCY CODE IS 3 LETTERS, E.G. ISK",
                Style::new().fg(th.accent),
            )),
        });
    }
    let above = lines.len() as u16;
    let list = Rect::new(
        inner.x,
        inner.y + above,
        inner.width,
        inner.height.saturating_sub(above),
    );
    // Scroll just far enough to keep the selection in view.
    let visible = usize::from(list.height).max(1);
    let offset = (sel + 1).saturating_sub(visible);
    for (i, choice) in hits.iter().enumerate().skip(offset).take(visible) {
        let selected = i == sel;
        let base = if selected {
            Style::new().bg(th.sel_bg).fg(th.bright).add_modifier(BOLD)
        } else {
            Style::new().fg(th.fg)
        };
        let current = if choice.code == app.cfg.currency {
            "IN USE"
        } else {
            ""
        };
        let row = format!(
            " {}  {:<4} {}  {current:<6} ",
            choice.code,
            choice.symbol,
            fmt::pad(
                &choice.name.to_uppercase(),
                width.saturating_sub(21),
                app.glyphs.ellipsis
            ),
        );
        lines.push(Line::from(vec![
            Span::styled(if selected { "►" } else { " " }, base.fg(th.accent)),
            Span::styled(fmt::pad(&row, width.saturating_sub(1), ""), base),
        ]));
    }
    f.render_widget(Paragraph::new(lines), inner);
    let shown = hits.len().saturating_sub(offset).min(visible) as u16;
    Regions {
        popup: outer,
        list: Rect {
            height: shown,
            ..list
        },
        list_offset: offset,
        ..Regions::default()
    }
}

// ----------------------------------------------------------------- calendar

fn calendar(f: &mut Frame, area: Rect, app: &App, ret: bool, cursor: NaiveDate) -> Regions {
    let th = app.theme;
    let today = Local::now().date_naive();
    let floor = if ret { app.form.depart } else { today };
    let (outer, inner) = open(
        f,
        area,
        app,
        if ret { "RETURN DATE" } else { "DEPARTURE DATE" },
        34,
        14,
    );
    let fares = app.fare_map().filter(|m| !m.is_empty());

    let first = cursor.with_day(1).unwrap_or(cursor);
    let lead = i64::from(first.weekday().num_days_from_monday());
    let month_title = cursor.format("%B %Y").to_string().to_uppercase();
    let mut lines = vec![
        Line::from(Span::styled(
            format!("◄ {month_title} ►"),
            Style::new().fg(th.bright).add_modifier(BOLD),
        ))
        .centered(),
        Line::from(Span::styled(
            "  MO  TU  WE  TH  FR  SA  SU",
            Style::new().fg(th.dim),
        )),
    ];
    // Six rows always, so the popup does not jump as months change length.
    for week in 0..6 {
        let mut spans = vec![Span::raw(" ")];
        for weekday in 0..7 {
            let date = first + Duration::days(week * 7 + weekday - lead);
            if date.month() != first.month() {
                spans.push(Span::raw("    "));
                continue;
            }
            // Days with a known fare take its heat colour: the calendar
            // doubles as a price map of the month.
            let fare = fares
                .as_ref()
                .and_then(|m| m.at(date).map(|(price, exact)| (m.heat(price), exact)));
            let mut style = match fare {
                _ if date < floor => Style::new().fg(th.faint),
                Some((heat, true)) => Style::new().fg(th.heat(heat)).add_modifier(BOLD),
                Some((heat, false)) => Style::new().fg(th.heat(heat)),
                None => Style::new().fg(th.fg),
            };
            let in_trip = app.form.round_trip && date >= app.form.depart && date <= app.form.ret;
            if in_trip && date != cursor {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            if date == cursor {
                spans.push(Span::styled(
                    format!("[{:>2}]", date.day()),
                    Style::new().fg(th.bg).bg(th.bright).add_modifier(BOLD),
                ));
            } else {
                spans.push(Span::styled(format!(" {:>2} ", date.day()), style));
            }
        }
        lines.push(Line::from(spans));
    }
    let grid = Rect::new(
        inner.x + 1,
        inner.y + 2,
        28.min(inner.width.saturating_sub(1)),
        6,
    );

    lines.push(Line::from(""));
    let ahead = (cursor - today).num_days();
    lines.push(Line::from(vec![
        Span::styled(
            format!(" {}", fmt::day(cursor)),
            Style::new().fg(th.bright).add_modifier(BOLD),
        ),
        Span::styled(
            format!("  IN {ahead} DAY{}", if ahead == 1 { "" } else { "S" }),
            Style::new().fg(th.dim),
        ),
    ]));
    let fare_line = match fares
        .as_ref()
        .and_then(|m| m.at(cursor).map(|(p, exact)| (p, exact, m.heat(p))))
    {
        Some((price, exact, heat)) => Line::from(vec![
            Span::styled(
                format!(" {}", fmt::money(price, &app.cfg.currency)),
                Style::new().fg(th.heat(heat)).add_modifier(BOLD),
            ),
            Span::styled(
                if exact {
                    "  LOWEST SEEN"
                } else {
                    "  ESTIMATED"
                },
                Style::new().fg(th.dim),
            ),
        ]),
        None if fares.is_some() => Line::from(Span::styled(
            " NO FARE LOGGED FOR THIS DAY",
            Style::new().fg(th.dim),
        )),
        None => Line::from(Span::styled(
            " COLOURS APPEAR AS FARES ARE SCANNED",
            Style::new().fg(th.dim),
        )),
    };
    lines.push(fare_line);
    f.render_widget(Paragraph::new(lines), inner);
    Regions {
        popup: outer,
        grid,
        ..Regions::default()
    }
}

// --------------------------------------------------------------------- text

fn text(
    f: &mut Frame,
    area: Rect,
    app: &App,
    kind: TextKind,
    input: &str,
    error: Option<&str>,
) -> Regions {
    let th = app.theme;
    let (title, prompts): (&str, Vec<String>) = match kind {
        TextKind::DepWindow => (
            "DEPARTURE TIME WINDOW",
            vec![
                "LOCAL TIME AT THE ORIGIN, 24-HOUR, AS HHMM-HHMM.".into(),
                "E.G. 0600-1130.  2200-0500 WRAPS PAST MIDNIGHT.  EMPTY = ANY TIME.".into(),
            ],
        ),
        TextKind::ArrWindow => (
            "ARRIVAL TIME WINDOW",
            vec![
                "LOCAL TIME AT THE DESTINATION, 24-HOUR, AS HHMM-HHMM.".into(),
                "E.G. 0800-1800.  EMPTY = ANY TIME.".into(),
            ],
        ),
        TextKind::Airlines => (
            "AIRLINE FILTER",
            vec![
                "ONLY SHOW THESE AIRLINES: 2-CHARACTER CODES, COMMA-SEPARATED.".into(),
                "E.G. BA,AF  OR  STAR_ALLIANCE / SKYTEAM / ONEWORLD.  EMPTY = ANY.".into(),
            ],
        ),
        TextKind::ApiKey => {
            let current = match (app.live, app.key_source) {
                (true, Some(KeySource::Flag)) => "LIVE, KEY FROM --key",
                (true, Some(KeySource::Env)) => "LIVE, KEY FROM ICARUS_API_KEY",
                (true, Some(KeySource::Baked)) => "LIVE, KEY BUILT INTO THIS COPY",
                (true, _) => "LIVE, KEY FROM THE CONFIG FILE",
                (false, _) => "SIMULATOR (NO KEY)",
            };
            (
                "API KEY",
                vec![
                    "SERPAPI KEY FOR LIVE GOOGLE FLIGHTS FARES. FREE TIER AT SERPAPI.COM.".into(),
                    "PASTE IT WITH CTRL+V OR A RIGHT-CLICK, THEN PRESS ENTER.".into(),
                    "IT IS CHECKED AT ONCE, FREE.  EMPTY + ENTER = BACK TO THE SIMULATOR.".into(),
                    format!("CURRENT SOURCE: {current}"),
                ],
            )
        }
    };
    let (outer, inner) = open(f, area, app, title, 74, prompts.len() as u16 + 6);
    let mut lines: Vec<Line> = prompts
        .into_iter()
        .map(|p| Line::from(Span::styled(format!(" {p}"), Style::new().fg(th.dim))))
        .collect();
    lines.push(Line::from(""));
    // Never echo a key in full: show only its last four characters.
    let shown = if kind == TextKind::ApiKey {
        let count = input.chars().count();
        let tail: String = input.chars().skip(count.saturating_sub(4)).collect();
        format!("{}{tail}", "•".repeat(count.saturating_sub(4).min(40)))
    } else {
        input.to_uppercase()
    };
    lines.push(Line::from(vec![
        Span::styled(" ► ", Style::new().fg(th.accent).add_modifier(BOLD)),
        Span::styled(shown, Style::new().fg(th.bright).add_modifier(BOLD)),
        caret(app),
    ]));
    lines.push(Line::from(""));
    if let Some(error) = error {
        lines.push(Line::from(Span::styled(
            format!(" ! {error}"),
            Style::new().fg(th.alert).add_modifier(BOLD),
        )));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    Regions {
        popup: outer,
        ..Regions::default()
    }
}

// ------------------------------------------------------------------ dossier

fn dossier(f: &mut Frame, area: Rect, app: &App, scroll: u16) -> Regions {
    let width = (area.width * 4 / 5).clamp(60, 96);
    let height = area.height.saturating_sub(4).max(10);
    let (outer, inner) = open(f, area, app, "DOSSIER", width, height);
    let lines = panels::dossier_lines(app, inner.width);
    // Clamp here and hand the result back, so scrolling past the end does
    // not bank up invisible offset that must be scrolled back through.
    let scroll = scroll.min((lines.len() as u16).saturating_sub(inner.height));
    f.render_widget(Paragraph::new(lines).scroll((scroll, 0)), inner);
    Regions {
        popup: outer,
        scroll: Some(scroll),
        ..Regions::default()
    }
}

// ------------------------------------------------------------------- survey

fn survey(f: &mut Frame, area: Rect, app: &App, horizon: usize) -> Regions {
    let th = app.theme;
    let (outer, inner) = open(f, area, app, "FARE SURVEY", 72, 13);
    let label = HORIZONS[horizon.min(YEAR)].0;
    let ranges = app.survey_ranges(horizon);
    let calls = ranges.len();
    let days: i64 = ranges
        .iter()
        .map(|(from, to)| (*to - *from).num_days() + 1)
        .sum();
    let row = |name: &str, value: String, style: Style| {
        Line::from(vec![
            Span::styled(format!(" {name:<9}"), Style::new().fg(th.dim)),
            Span::styled(value, style),
        ])
    };
    let strong = Style::new().fg(th.bright).add_modifier(BOLD);
    let cost = if !app.live {
        ("FREE (SIMULATOR)".to_string(), strong)
    } else {
        let left = app.quota.map(|q| q.left);
        let short = left.is_some_and(|l| (l as usize) < calls);
        let fuel = left.map_or(String::new(), |l| format!("  (FUEL LEFT: {l})"));
        (
            format!(
                "{calls} API CALL{}{fuel}",
                if calls == 1 { "" } else { "S" }
            ),
            if short {
                Style::new().fg(th.alert).add_modifier(BOLD)
            } else {
                strong
            },
        )
    };
    let scope = if calls == 0 {
        "NONE LEFT TO PRICE: ALL DONE WITHIN THE LAST DAY".to_string()
    } else {
        format!("{days}, EVERY ONE PRICED")
    };
    let dim = Style::new().fg(th.dim);
    let lines = vec![
        Line::from(Span::styled(
            " PRICE EVERY DAY OF THE COMING MONTHS TO FILL THE HEATMAP.",
            Style::new().fg(th.fg),
        )),
        Line::from(Span::styled(
            " ONE CALL RETURNS THE LOWEST FARE FOR EACH DAY OF TWO MONTHS.",
            dim,
        )),
        Line::from(""),
        row(
            "HORIZON",
            format!("◄ {label} ►"),
            Style::new().fg(th.accent).add_modifier(BOLD),
        ),
        row("DAYS", scope, strong),
        row("COST", cost.0, cost.1),
        Line::from(""),
        Line::from(Span::styled(
            " NOTHING IS ESTIMATED: EACH MONTH'S CHEAPEST AND DEAREST DAY IS A",
            dim,
        )),
        Line::from(Span::styled(
            " FARE THAT WAS SEEN. MONTHS PRICED WITHIN THE LAST DAY ARE SKIPPED.",
            dim,
        )),
        Line::from(""),
        Line::from(Span::styled(
            " ENTER  LAUNCH        ←→  HORIZON        ESC  CANCEL",
            Style::new().fg(th.accent),
        )),
    ];
    f.render_widget(Paragraph::new(lines), inner);
    Regions {
        popup: outer,
        ..Regions::default()
    }
}

// ----------------------------------------------------------------- settings

/// The settings file, laid out for changing. Every row is something the
/// file holds; the foot of the box says where that file is and when it was
/// last written.
fn settings(f: &mut Frame, area: Rect, app: &App, sel: usize) -> Regions {
    let th = app.theme;
    let (outer, inner) = open(f, area, app, "SETTINGS", 80, 20);
    let width = usize::from(inner.width);
    let dim = Style::new().fg(th.dim);
    let mut lines = vec![Line::from("")];
    for (i, row) in Setting::ALL.iter().enumerate() {
        let selected = i == sel;
        let base = if selected {
            Style::new().bg(th.sel_bg).fg(th.bright).add_modifier(BOLD)
        } else {
            Style::new().fg(th.fg)
        };
        let (value, tone) = setting_value(app, *row);
        let label = format!(" {:<14}", row.label());
        let rest = width.saturating_sub(1 + label.chars().count());
        lines.push(Line::from(vec![
            Span::styled(if selected { "►" } else { " " }, base.fg(th.accent)),
            Span::styled(label, base),
            Span::styled(
                fmt::pad(&value, rest, app.glyphs.ellipsis),
                tone.map_or(base, |color| base.fg(color)),
            ),
        ]));
    }
    let list = Rect::new(
        inner.x,
        inner.y + 1,
        inner.width,
        (Setting::ALL.len() as u16).min(inner.height.saturating_sub(1)),
    );

    let [hint_a, hint_b] = setting_hint(Setting::ALL[sel.min(Setting::ALL.len() - 1)]);
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(format!(" {hint_a}"), dim)));
    lines.push(Line::from(Span::styled(format!(" {hint_b}"), dim)));
    lines.push(Line::from(""));
    match &app.saved {
        Some((path, at)) => {
            // A long path keeps its end: the file's name is the part that matters.
            let place = keep_end(&path.display().to_string(), width.saturating_sub(9));
            lines.push(Line::from(vec![
                Span::styled(" FILE    ", dim),
                Span::styled(place, Style::new().fg(th.bright)),
            ]));
            lines.push(Line::from(Span::styled(
                format!(
                    "         SAVED AT {}. EVERY CHANGE IS SAVED AS IT IS MADE.",
                    at.format("%H:%M:%S")
                ),
                dim,
            )));
        }
        None => {
            lines.push(Line::from(vec![
                Span::styled(" FILE    ", dim),
                Span::styled(
                    "NOT WRITTEN: THIS IS A SCRIPTED RUN",
                    Style::new().fg(th.accent),
                ),
            ]));
            lines.push(Line::from(""));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " ↑↓ CHOOSE   ←→ CHANGE   ENTER EDIT   T TEST KEY   S SAVE NOW   ESC CLOSE",
        Style::new().fg(th.accent),
    )));
    f.render_widget(Paragraph::new(lines), inner);
    Regions {
        popup: outer,
        list,
        ..Regions::default()
    }
}

/// A setting's value as shown, and a colour for it when it carries news.
fn setting_value(app: &App, row: Setting) -> (String, Option<Color>) {
    let th = app.theme;
    match row {
        Setting::ApiKey => {
            if app.key_state == KeyState::Absent {
                return (
                    "NONE - FARES ARE SIMULATED. PRESS ENTER TO ADD ONE".into(),
                    Some(th.accent),
                );
            }
            let from = match app.key_source {
                Some(KeySource::Flag) => " (FROM --key)",
                Some(KeySource::Env) => " (FROM ICARUS_API_KEY)",
                Some(KeySource::Baked) => " (BUILT IN)",
                _ => "",
            };
            let (state, tone) = match &app.key_state {
                KeyState::Checking => ("CHECKING...".to_string(), th.accent),
                KeyState::Accepted => (
                    match app.quota {
                        Some(q) => format!("ACCEPTED. {} OF {} SEARCHES LEFT", q.left, q.per_month),
                        None => "ACCEPTED".to_string(),
                    },
                    th.good,
                ),
                KeyState::Rejected(_) => {
                    ("REJECTED BY SERPAPI. ENTER IT AGAIN".to_string(), th.alert)
                }
                KeyState::Unchecked(_) => (
                    "NOT CHECKED: SERPAPI COULD NOT BE REACHED".to_string(),
                    th.accent,
                ),
                KeyState::Absent => (String::new(), th.fg),
            };
            // Never the key itself: its last four characters tell two apart.
            (format!("••••{}{from}  {state}", app.key_tail), Some(tone))
        }
        Setting::Currency => {
            let name = currency::get(&app.cfg.currency)
                .map_or("AS TYPED", |c| c.name)
                .to_uppercase();
            (format!("{}  {name}", app.cfg.currency), None)
        }
        Setting::Theme => (app.theme.name.to_uppercase(), None),
        Setting::Charset => (app.cfg.charset.to_uppercase(), None),
        Setting::Continents => (
            if app.cfg.continents {
                "EACH ITS OWN COLOUR"
            } else {
                "ALL ONE COLOUR"
            }
            .into(),
            None,
        ),
        Setting::Borders => (
            if app.cfg.borders { "SHOWN" } else { "HIDDEN" }.into(),
            None,
        ),
        Setting::TextSize => match app.cfg.text_size {
            0 => ("AS THE CONSOLE HAS IT".into(), None),
            pixels => (format!("{pixels} PIXELS HIGH"), None),
        },
        Setting::CacheHours => {
            let hours = app.cfg.cache_hours;
            (
                format!("{hours} HOUR{}", if hours == 1 { "" } else { "S" }),
                None,
            )
        }
    }
}

/// Two lines on what the selected setting does and how to change it.
fn setting_hint(row: Setting) -> [&'static str; 2] {
    match row {
        Setting::ApiKey => [
            "ENTER, THEN PASTE YOUR SERPAPI KEY WITH CTRL+V OR A RIGHT-CLICK. A KEY IS",
            "FREE AT SERPAPI.COM (250 SEARCHES A MONTH) AND IS CHECKED AT ONCE.",
        ],
        Setting::Currency => [
            "ENTER TO PICK FROM THE LIST, OR TYPE ANY 3-LETTER CODE.",
            "FARES ARE FETCHED IN IT, NEVER CONVERTED.",
        ],
        Setting::Theme => ["LEFT OR RIGHT CHANGES THE COLOUR SCHEME.", ""],
        Setting::Charset => [
            "AUTO WORKS OUT WHAT THIS CONSOLE CAN DRAW. SAFE AVOIDS BRAILLE, FOR OLD",
            "FONTS. TAKES EFFECT THE NEXT TIME THE PROGRAM STARTS.",
        ],
        Setting::Continents => [
            "LEFT OR RIGHT: A COLOUR FOR EACH CONTINENT ON THE MAP, OR ONE FOR ALL.",
            "",
        ],
        Setting::Borders => [
            "LEFT OR RIGHT: DRAW COUNTRY BORDERS ON THE MAP, OR LEAVE THEM OUT.",
            "",
        ],
        Setting::TextSize => [
            "LEFT FOR SMALLER TEXT AND A FINER MAP, RIGHT FOR LARGER. THE WINDOW KEEPS",
            "ITS SIZE. WINDOWS CONSOLE ONLY; ELSEWHERE USE CTRL AND MINUS.",
        ],
        Setting::CacheHours => [
            "HOW LONG A LIVE SEARCH IS REUSED BEFORE IT IS ASKED AGAIN. LONGER SPENDS",
            "FEWER SEARCHES, SHORTER IS FRESHER. TAKES EFFECT NEXT START.",
        ],
    }
}

/// The last `width` characters of `text`, marked as cut when they are.
fn keep_end(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count <= width {
        return text.to_string();
    }
    let kept: String = text.chars().skip(count - width.saturating_sub(3)).collect();
    format!("...{kept}")
}

// -------------------------------------------------------------------- guide

/// How to read the fare-intel screen, in full.
fn guide(f: &mut Frame, area: Rect, app: &App) -> Regions {
    let th = app.theme;
    let lines = intel::guide_lines(th, true);
    let (outer, inner) = open(
        f,
        area,
        app,
        "HOW TO READ FARE INTEL",
        78,
        lines.len() as u16 + 2,
    );
    f.render_widget(Paragraph::new(lines), inner);
    Regions {
        popup: outer,
        ..Regions::default()
    }
}

// --------------------------------------------------------------------- help

fn help(f: &mut Frame, area: Rect, app: &App) -> Regions {
    let th = app.theme;
    let (outer, inner) = open(f, area, app, "FIELD MANUAL", 92, 34);
    let head = |text: &str| {
        Line::from(Span::styled(
            format!(" {text}"),
            Style::new().fg(th.accent).add_modifier(BOLD),
        ))
    };
    let key = |keys: &str, what: &str| {
        Line::from(vec![
            Span::styled(
                format!("   {keys:<16}"),
                Style::new().fg(th.bright).add_modifier(BOLD),
            ),
            Span::styled(what.to_string(), Style::new().fg(th.fg)),
        ])
    };
    let lines = vec![
        head("FIND A FLIGHT"),
        key("S", "SCAN: SEARCH THE ROUTE (AND NEARBY DATES IF FLEX > 0)"),
        key(
            "ENTER",
            "ON A TRACK: ROUND TRIP - PICK ITS RETURN; OTHERWISE FULL DOSSIER",
        ),
        key("SPACE", "FULL-SCREEN DOSSIER FOR THE SELECTED TRACK"),
        key("O", "OPEN THIS SEARCH IN YOUR BROWSER TO BOOK"),
        key(
            "1 2 3 4",
            "SORT BY PRICE / BEST VALUE / DURATION / DEPARTURE (AGAIN: REVERSE)",
        ),
        key(
            "5 6 7",
            "SORT BY ARRIVAL / STOPS / CARRIER - OR CLICK ANY COLUMN HEADING",
        ),
        key("[  ]", "PREVIOUS / NEXT SCANNED DATE"),
        key("X", "SWAP ORIGIN AND DESTINATION"),
        head("FARE INTEL"),
        key(
            "I",
            "THE YEAR'S FARES, EACH MONTH'S BEST AND WORST DAY, PRICE HISTORY",
        ),
        key(
            "V",
            "SURVEY: PRICE EVERY DAY OF THE COMING MONTHS (A YEAR IS 6 CALLS)",
        ),
        head("MOVE AROUND"),
        key("TAB", "NEXT PANE (SHIFT+TAB: PREVIOUS)"),
        key(
            "ARROWS / HJKL",
            "MOVE WITHIN A PANE; LEFT/RIGHT CHANGES A FORM VALUE",
        ),
        key("ENTER", "ON A FORM FIELD: OPEN ITS EDITOR"),
        key("BACKSPACE", "CLEAR A FORM FIELD"),
        key("ESC", "CLOSE / GO BACK"),
        head("GLOBE"),
        key("CLICK", "SET DESTINATION (FIRST CLICK SETS ORIGIN)"),
        key("RIGHT-CLICK", "SET ORIGIN"),
        key("DRAG / WHEEL", "ROTATE / ZOOM TOWARDS THE POINTER"),
        key(
            "G  F  C",
            "GLOBE OR FLAT MAP / FULLSCREEN / CENTRE ON ROUTE",
        ),
        key(
            "+  -  B  P",
            "ZOOM IN / OUT / COUNTRY BORDERS / CONTINENT COLOURS",
        ),
        key(
            ">  <",
            "FINER / COARSER MAP: SMALLER OR LARGER TEXT (WINDOWS CONSOLE)",
        ),
        key(
            "M",
            "DRAWING STYLE: BRAILLE / LINE ART / BLOCKS (IF THE MAP LOOKS BROKEN)",
        ),
        head("SETTINGS"),
        key(
            "F2",
            "ALL SETTINGS: API KEY, CURRENCY, THEME... (OR CLICK IT IN THE TOP BAR)",
        ),
        key(
            "SHIFT+K",
            "STRAIGHT TO THE API KEY BOX. PASTE WITH CTRL+V OR A RIGHT-CLICK",
        ),
        key(
            "$   T",
            "CURRENCY (OR CLICK IT IN THE TOP BAR) / COLOUR THEME",
        ),
        key("Q", "QUIT (CTRL+C ALSO WORKS)"),
        Line::from(""),
        Line::from(Span::styled(
            " MOUSE: CLICK SELECTS, DOUBLE-CLICK OPENS, WHEEL SCROLLS.",
            Style::new().fg(th.dim),
        )),
    ];
    f.render_widget(Paragraph::new(lines), inner);
    Regions {
        popup: outer,
        ..Regions::default()
    }
}
