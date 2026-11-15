//! Rendering. One call to `draw` paints a whole frame from `App` state, and
//! records in `App::hit` where each clickable thing ended up.

pub mod canvas;
pub mod globe;
mod intel;
mod panels;
mod popups;
pub mod theme;

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph},
};

use crate::app::{App, Hit};
use theme::Theme;

pub const MIN_WIDTH: u16 = 96;
pub const MIN_HEIGHT: u16 = 24;
/// Terminal width at which the dossier gets a column of its own. Below
/// this the tracks table would be squeezed, and the table matters more.
const WIDE: u16 = 168;

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let th = app.theme;
    // Paint our own background: a retro terminal is black whatever the
    // user's colour scheme says.
    f.buffer_mut()
        .set_style(area, Style::new().fg(th.fg).bg(th.bg));
    app.hit = Hit::default();

    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        let lines = vec![
            Line::from(Span::styled(
                "ICARUS-SEARCH",
                Style::new().fg(th.bright).add_modifier(Modifier::BOLD),
            )),
            Line::from(format!("WINDOW TOO SMALL ({}x{})", area.width, area.height)),
            Line::from(format!("ENLARGE IT TO AT LEAST {MIN_WIDTH}x{MIN_HEIGHT}")),
        ];
        let height = 3.min(area.height);
        let y = area.y + (area.height - height) / 2;
        f.render_widget(
            Paragraph::new(lines).centered(),
            Rect::new(area.x, y, area.width, height),
        );
        return;
    }

    let [head, body, foot] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);
    panels::header(f, head, app);

    if app.intel.is_some() {
        intel::draw(f, body, app);
    } else if app.globe_full {
        globe::draw(f, body, app);
    } else {
        let wide = body.width >= WIDE;
        let columns = if wide {
            Layout::horizontal([
                Constraint::Length(36),
                Constraint::Min(40),
                Constraint::Length(48),
            ])
            .split(body)
        } else {
            Layout::horizontal([Constraint::Length(36), Constraint::Min(40)]).split(body)
        };
        let (left, mid) = (columns[0], columns[1]);
        let [form, radar] =
            Layout::vertical([Constraint::Length(15.min(left.height)), Constraint::Min(0)])
                .areas(left);
        // The table takes two fifths of the height, but never more than it
        // can use: in a tall window the spare rows go to the map.
        let tracks_height = (mid.height * 2 / 5)
            .clamp(8, 18)
            .min(mid.height.saturating_sub(6));
        let [globe_area, tracks] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(tracks_height)]).areas(mid);
        panels::form(f, form, app);
        panels::radar(f, radar, app);
        globe::draw(f, globe_area, app);
        panels::tracks(f, tracks, app);
        if wide {
            panels::dossier(f, columns[2], app);
        }
    }

    panels::footer(f, foot, app);
    popups::draw(f, area, app);
}

/// A titled, bordered pane. The focused pane gets a double border and an
/// inverted title, so it is obvious where the keyboard is.
pub fn pane<'a>(title: impl Into<String>, focused: bool, th: Theme) -> Block<'a> {
    pane_with(
        Line::from(Span::styled(
            format!(" {} ", title.into()),
            title_style(focused, th),
        )),
        focused,
        th,
    )
}

/// A pane whose title is made of several differently styled pieces.
pub fn pane_with<'a>(title: Line<'a>, focused: bool, th: Theme) -> Block<'a> {
    Block::bordered()
        .border_type(if focused {
            BorderType::Double
        } else {
            BorderType::Plain
        })
        .border_style(
            Style::new()
                .fg(if focused { th.bright } else { th.dim })
                .bg(th.bg),
        )
        .style(Style::new().fg(th.fg).bg(th.bg))
        .title(title)
}

pub fn title_style(focused: bool, th: Theme) -> Style {
    if focused {
        Style::new()
            .fg(th.bg)
            .bg(th.bright)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(th.fg).add_modifier(Modifier::BOLD)
    }
}

/// A solid tag: dark text on a block of colour. For the few facts that must
/// not be missed, like whether a price is for one way or both.
pub fn chip(text: &str, color: Color, th: Theme) -> Span<'static> {
    Span::styled(
        format!(" {text} "),
        Style::new()
            .fg(th.bg)
            .bg(color)
            .add_modifier(Modifier::BOLD),
    )
}

/// The trip-type tag, the same wherever it appears.
pub fn trip_chip(round_trip: bool, th: Theme) -> Span<'static> {
    chip(
        if round_trip { "ROUND-TRIP" } else { "ONE-WAY" },
        th.trip(round_trip),
        th,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::Options,
        config::{Config, Session},
    };
    use ratatui::{Terminal, backend::TestBackend};

    /// Drive the app with a script, then render one frame as plain text.
    fn screen(width: u16, height: u16, script: &str) -> String {
        let opts = Options {
            key: None,
            demo: true,
            latency: false,
            ephemeral: true,
            charset: Some("rich".into()),
            dump: None,
        };
        let mut app = App::new(Config::default(), Session::default(), opts);
        script.split_whitespace().for_each(|token| app.feed(token));
        app.settle();
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let buf = terminal.backend().buffer();
        (0..height)
            .map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn every_screen_renders_at_every_supported_size() {
        // The point is "no panic, no arithmetic overflow" across layouts.
        let scripts = [
            "",
            "from:LHR to:NRT s wait",
            "from:LHR to:NRT s wait enter wait space",
            "from:JFK to:SYD s wait f",
            "from:JFK to:SYD s wait g",
            "from:LHR to:NRT i",
            "from:LHR to:NRT v enter wait",
            "?",
            "down enter text:par",
            "down down down enter",
            "K",
            "f2",
            "f2 down down down down down down down",
            "from:LHR to:NRT v",
        ];
        for (w, h) in [
            (96, 24),
            (120, 30),
            (160, 45),
            (220, 60),
            (40, 10),
            (96, 23),
        ] {
            for script in scripts {
                let text = screen(w, h, script);
                assert_eq!(text.lines().count(), h as usize, "{w}x{h} {script:?}");
            }
        }
    }

    #[test]
    fn main_screen_shows_route_results_and_simulation_warning() {
        let text = screen(140, 40, "from:LHR to:NRT s wait");
        for needle in [
            "ICARUS-SEARCH",
            "SIMULATED DATA",
            "MISSION PARAMETERS",
            "LHR LONDON",
            "PRICE RADAR",
            "TRACKS",
            "GLOBE",
            "NRT",
            "SCAN COMPLETE",
        ] {
            assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
        }
    }

    // ---- mouse -----------------------------------------------------------
    // These drive real mouse events against a real rendered layout, because
    // hit-testing is only meaningful against where things were drawn.

    use crate::{
        airports,
        app::{KeyState, Pane, Popup, Sort},
        geo::Side,
        provider::Quota,
    };
    use ratatui::{
        crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind},
        layout::Position,
    };

    fn rendered(width: u16, height: u16, script: &str) -> App {
        let opts = Options {
            key: None,
            demo: true,
            latency: false,
            ephemeral: true,
            charset: Some("rich".into()),
            dump: None,
        };
        let mut app = App::new(Config::default(), Session::default(), opts);
        script.split_whitespace().for_each(|token| app.feed(token));
        app.settle();
        redraw(&mut app, width, height);
        // The first frame tells the app its pane size; let it re-frame.
        app.settle();
        redraw(&mut app, width, height);
        app
    }

    fn redraw(app: &mut App, width: u16, height: u16) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
    }

    /// Redraw and return the screen as text.
    fn screen_of(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buf = terminal.backend().buffer();
        (0..height)
            .map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn mouse(app: &mut App, kind: MouseEventKind, x: u16, y: u16) {
        app.on_mouse(MouseEvent {
            kind,
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        });
    }

    fn click(app: &mut App, x: u16, y: u16) {
        mouse(app, MouseEventKind::Down(MouseButton::Left), x, y);
        mouse(app, MouseEventKind::Up(MouseButton::Left), x, y);
    }

    /// The terminal cell an airport is drawn in.
    fn cell_of(app: &App, iata: &str) -> (u16, u16) {
        let inner = app.hit.globe_inner;
        let (w, h) = (f64::from(inner.width) * 2.0, f64::from(inner.height) * 4.0);
        let (x, y, side) = app
            .view
            .project(airports::db().get(iata).unwrap().vec, w, h);
        assert_eq!(side, Side::Front, "{iata} should be on the visible side");
        (inner.x + (x / 2.0) as u16, inner.y + (y / 4.0) as u16)
    }

    /// A click that covers several airports opens the picker; choose from it.
    fn choose(app: &mut App, iata: &str) {
        if let Some(Popup::Airport { hits, sel, .. }) = &mut app.popup {
            *sel = hits
                .iter()
                .position(|a| a.iata == iata)
                .unwrap_or_else(|| panic!("{iata} should be among the candidates offered"));
            app.feed("enter");
        }
    }

    #[test]
    fn single_click_selects_a_track_and_double_click_opens_it() {
        let mut app = rendered(120, 30, "from:LHR to:NRT s wait");
        let rows = app.hit.track_rows;
        assert!(rows.height >= 4, "tracks table should have rows on screen");
        click(&mut app, rows.x + 5, rows.y + 2);
        assert_eq!(app.sel(), 2);
        assert_eq!(app.focus, Pane::Tracks);
        assert!(
            app.board.ret.is_none(),
            "one click must not spend a search on return flights"
        );
        click(&mut app, rows.x + 5, rows.y + 2);
        assert!(app.board.ret.is_some(), "a double-click opens the track");
    }

    #[test]
    fn two_clicks_on_different_rows_are_not_a_double_click() {
        let mut app = rendered(120, 30, "from:LHR to:NRT s wait");
        let rows = app.hit.track_rows;
        click(&mut app, rows.x + 5, rows.y + 1);
        click(&mut app, rows.x + 5, rows.y + 3);
        assert_eq!(app.sel(), 3);
        assert!(app.board.ret.is_none());
    }

    #[test]
    fn wheel_scrolls_the_list_under_the_pointer_and_is_clamped() {
        let mut app = rendered(120, 30, "from:LHR to:NRT s wait");
        let inside = Position::new(app.hit.track_rows.x + 3, app.hit.track_rows.y + 1);
        // A touchpad can deliver dozens of wheel events in one frame.
        app.on_scroll(inside, 40);
        assert_eq!(app.sel(), 4, "one frame of scrolling moves at most 4 rows");
        app.on_scroll(inside, -40);
        assert_eq!(app.sel(), 0);
        app.on_scroll(inside, -40);
        assert_eq!(app.sel(), 0, "scrolling past the top stays at the top");
        // Over the form, the wheel moves the form cursor instead.
        let form = Position::new(app.hit.form_rows.x + 2, app.hit.form_rows.y + 2);
        app.on_scroll(form, 1);
        assert_eq!((app.form.cursor, app.sel()), (1, 0));
    }

    #[test]
    fn wheel_over_the_globe_zooms_about_the_pointer() {
        let mut app = rendered(120, 30, "from:LHR to:NRT s wait");
        let (x, y) = cell_of(&app, "LHR");
        let before = app.view.zoom;
        app.on_scroll(Position::new(x, y), -1);
        assert!(app.view.zoom > before, "wheel up zooms in");
        redraw(&mut app, 120, 30);
        let (x2, y2) = cell_of(&app, "LHR");
        assert!(
            x.abs_diff(x2) <= 1 && y.abs_diff(y2) <= 1,
            "LHR stays under the pointer: ({x},{y}) -> ({x2},{y2})"
        );
    }

    #[test]
    fn dragging_the_globe_rotates_it_without_picking_an_airport() {
        let mut app = rendered(120, 30, "from:LHR to:NRT s wait");
        let (x, y) = cell_of(&app, "NRT");
        let (lon, dest) = (app.view.lon0, app.form.dest.map(|a| a.iata));
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x - 6, y);
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), x - 2, y);
        // The pointer may leave the pane mid-drag; the globe keeps following.
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 0, 0);
        mouse(&mut app, MouseEventKind::Up(MouseButton::Left), 0, 0);
        assert_ne!(app.view.lon0, lon, "the view rotated");
        assert_eq!(app.form.dest.map(|a| a.iata), dest, "a drag is not a click");
    }

    #[test]
    fn clicking_airports_on_the_globe_builds_a_route() {
        // Zoomed out and zoomed in, globe and flat map: a click on the cell
        // an airport is drawn in must find that airport's city.
        for script in [
            "from:LHR to:NRT s wait",
            "from:LHR to:NRT s wait g wait",
            "from:LHR to:CDG",
            "from:JFK to:SYD",
        ] {
            let mut app = rendered(120, 30, script);
            let (from, to) = (app.form.origin.unwrap(), app.form.dest.unwrap());
            let (ox, oy) = cell_of(&app, from.iata);
            let (dx, dy) = cell_of(&app, to.iata);
            app.form.origin = None;
            app.form.dest = None;
            click(&mut app, ox, oy);
            choose(&mut app, from.iata);
            assert_eq!(
                app.form.origin.map(|a| a.iata),
                Some(from.iata),
                "{script}: first click sets the origin"
            );
            click(&mut app, dx, dy);
            choose(&mut app, to.iata);
            assert_eq!(
                app.form.dest.map(|a| a.iata),
                Some(to.iata),
                "{script}: second click sets the destination"
            );
            // Right-click re-targets the origin, but not onto the destination.
            mouse(&mut app, MouseEventKind::Down(MouseButton::Right), dx, dy);
            choose(&mut app, to.iata);
            assert_eq!(
                app.form.origin.map(|a| a.iata),
                Some(from.iata),
                "{script}: an airport cannot be both ends"
            );
        }
    }

    #[test]
    fn a_click_covering_several_airports_asks_rather_than_guesses() {
        // At this zoom all of London's airports sit inside one character.
        let mut app = rendered(120, 30, "from:LHR to:NRT s wait");
        let (x, y) = cell_of(&app, "LHR");
        click(&mut app, x, y);
        let Some(Popup::Airport { hits, .. }) = &app.popup else {
            panic!("expected the airport picker")
        };
        let codes: Vec<_> = hits.iter().map(|a| a.iata).collect();
        assert!(codes.contains(&"LHR") && codes.len() > 1, "{codes:?}");
        // The picker's rows are clickable where they are drawn.
        redraw(&mut app, 120, 30);
        let row = codes
            .iter()
            .position(|c| *c == "LGW")
            .expect("Gatwick is a candidate") as u16;
        let list = app.hit.popup_list;
        click(&mut app, list.x + 4, list.y + row);
        click(&mut app, list.x + 4, list.y + row);
        assert!(app.popup.is_none());
        assert_eq!(app.form.dest.map(|a| a.iata), Some("LGW"));
    }

    #[test]
    fn a_lost_button_release_does_not_leave_the_globe_stuck_to_the_pointer() {
        let mut app = rendered(120, 30, "from:LHR to:NRT s wait");
        let (x, y) = cell_of(&app, "NRT");
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x - 6, y);
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), x - 3, y);
        // The button is released outside the window, so no Up ever arrives.
        // The next plain movement proves the button is no longer held.
        mouse(&mut app, MouseEventKind::Moved, x, y);
        let lon = app.view.lon0;
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), x + 9, y);
        assert_eq!(app.view.lon0, lon);
    }

    /// Where the planet's centre is drawn, in dots of the globe's canvas.
    fn planet(app: &App) -> (f64, f64) {
        let inner = app.hit.globe_inner;
        app.view
            .globe_centre(f64::from(inner.width) * 2.0, f64::from(inner.height) * 4.0)
    }

    fn near(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6
    }

    /// A left-button drag across the globe with a modifier key held.
    fn drag_with(app: &mut App, held: KeyModifiers, from: (u16, u16), to: (u16, u16)) {
        let mut send = |kind, (column, row): (u16, u16)| {
            app.on_mouse(MouseEvent {
                kind,
                column,
                row,
                modifiers: held,
            });
        };
        send(MouseEventKind::Down(MouseButton::Left), from);
        send(MouseEventKind::Drag(MouseButton::Left), to);
        send(MouseEventKind::Up(MouseButton::Left), to);
    }

    #[test]
    fn the_globe_stays_in_the_middle_of_its_pane_through_framing_zooming_and_turning() {
        for script in [
            "",
            "from:LHR to:NRT s wait",
            "from:LHR to:CDG",
            "from:SYD to:PER s wait",
        ] {
            let mut app = rendered(120, 30, script);
            let inner = app.hit.globe_inner;
            let middle = (f64::from(inner.width), f64::from(inner.height) * 2.0);
            assert_eq!(planet(&app), middle, "{script:?}: as first framed");

            // The wheel, over the planet and then over empty space.
            let over_planet = Position::new(inner.x + inner.width / 2 + 4, inner.y + 3);
            let over_space = Position::new(inner.x + 1, inner.y + 1);
            for pos in [over_planet, over_space] {
                app.on_scroll(pos, -2);
                app.on_scroll(pos, 1);
            }
            assert_eq!(planet(&app), middle, "{script:?}: after zooming");

            // Turned by hand, then framed afresh.
            let from = (inner.x + 20, inner.y + 5);
            drag_with(&mut app, KeyModifiers::NONE, from, (from.0 + 6, from.1 + 2));
            assert_eq!(planet(&app), middle, "{script:?}: after a drag");
            app.feed("c");
            app.feed("wait");
            assert_eq!(planet(&app), middle, "{script:?}: after framing again");
        }
    }

    #[test]
    fn shift_drag_moves_the_globe_and_it_stays_where_it_is_put() {
        // Ctrl works too: many terminals keep Shift+drag for selecting text.
        for held in [KeyModifiers::SHIFT, KeyModifiers::CONTROL] {
            let mut app = rendered(120, 30, "from:LHR to:NRT s wait");
            let inner = app.hit.globe_inner;
            let from = (inner.x + inner.width / 2, inner.y + inner.height / 2);
            let start = planet(&app);
            let facing = (app.view.lat0, app.view.lon0, app.view.zoom);

            drag_with(&mut app, held, from, (from.0 + 7, from.1 + 3));
            // Seven cells right and three down; a cell is 2 by 4 dots.
            let moved = planet(&app);
            assert!(
                near(moved, (start.0 + 14.0, start.1 + 12.0)),
                "{held:?}: {start:?} -> {moved:?}"
            );
            assert_eq!(
                (app.view.lat0, app.view.lon0, app.view.zoom),
                facing,
                "{held:?}: moving the globe does not turn or resize it"
            );
            assert_eq!(
                app.form.dest.map(|a| a.iata),
                Some("NRT"),
                "a drag is not a click"
            );

            // Zooming and framing a route leave it there, and the route is
            // found in the pane all the same.
            app.on_scroll(Position::new(from.0, from.1), -1);
            app.feed("c");
            app.feed("wait");
            assert!(near(planet(&app), moved), "{held:?}: {:?}", planet(&app));
            redraw(&mut app, 120, 30);
            for iata in ["LHR", "NRT"] {
                let (x, y) = cell_of(&app, iata);
                assert!(
                    inner.contains(Position::new(x, y)),
                    "{held:?}: {iata} is drawn at ({x}, {y}), outside {inner:?}"
                );
            }

            // 0 puts it back.
            app.feed("0");
            assert!(near(planet(&app), start));
        }
    }

    #[test]
    fn shift_with_the_arrows_moves_the_globe_and_bare_arrows_turn_it() {
        let mut app = rendered(120, 30, "from:LHR to:NRT s wait");
        while app.focus != Pane::Globe {
            app.feed("tab");
        }
        let (start, lon) = (planet(&app), app.view.lon0);
        app.feed("shift+right");
        app.feed("shift+down");
        let moved = planet(&app);
        assert!(
            moved.0 > start.0 && moved.1 > start.1,
            "{start:?} -> {moved:?}"
        );
        assert_eq!(app.view.lon0, lon, "moving is not turning");
        app.feed("right");
        assert_ne!(app.view.lon0, lon, "a bare arrow still turns it");
        assert!(near(planet(&app), moved), "turning is not moving");
        // Nothing else was touched along the way.
        assert_eq!(app.cfg.theme, Config::default().theme);
        assert!(app.intel.is_none() && app.popup.is_none() && !app.globe_full);
    }

    #[test]
    fn on_the_flat_map_a_shift_drag_is_an_ordinary_drag() {
        let mut app = rendered(120, 30, "from:LHR to:NRT s wait g wait");
        let inner = app.hit.globe_inner;
        let from = (inner.x + inner.width / 2, inner.y + inner.height / 2);
        let lon = app.view.lon0;
        drag_with(&mut app, KeyModifiers::SHIFT, from, (from.0 + 7, from.1));
        assert_ne!(app.view.lon0, lon, "the map scrolled");
        assert_eq!((app.view.drop, app.view.shift), (0.0, 0.0));
    }

    #[test]
    fn clicking_outside_a_popup_dismisses_it_and_inside_does_not() {
        let mut app = rendered(120, 30, "?");
        let popup = app.hit.popup;
        click(&mut app, popup.x + 2, popup.y + 2);
        assert!(app.popup.is_some());
        click(&mut app, 0, 0);
        assert!(app.popup.is_none());
    }

    #[test]
    fn clicking_a_column_heading_sorts_by_it_and_again_reverses() {
        let mut app = rendered(140, 40, "from:LHR to:NRT s wait");
        let head = app.hit.track_head;
        assert_eq!(head.height, 1, "the heading row is on screen");
        // Headings are in TRACK_COLUMNS order; the fourth is TIME.
        let time = app.hit.track_cols[3].0 + 1;
        click(&mut app, time, head.y);
        assert_eq!(
            (app.board.sort, app.board.descending),
            (Sort::Duration, false)
        );
        let minutes: Vec<u32> = app.tracks().iter().map(|it| it.total_min).collect();
        assert!(minutes.windows(2).all(|w| w[0] <= w[1]), "{minutes:?}");
        redraw(&mut app, 140, 40);
        click(&mut app, time, head.y);
        assert_eq!(
            (app.board.sort, app.board.descending),
            (Sort::Duration, true)
        );
        // The heading in force wears the arrow.
        redraw(&mut app, 140, 40);
        let price = app.hit.track_cols[0].0 + 4;
        click(&mut app, price, head.y);
        assert_eq!((app.board.sort, app.board.descending), (Sort::Price, false));
        assert!(screen_of(&mut app, 140, 40).contains("PRICE▲"));
    }

    #[test]
    fn clicking_the_currency_in_the_top_bar_opens_the_picker() {
        let mut app = rendered(140, 40, "");
        let button = app.hit.currency;
        assert!(
            button.width >= 5,
            "the currency button is on screen: {button:?}"
        );
        click(&mut app, button.x + 1, button.y);
        assert!(matches!(app.popup, Some(Popup::Currency { .. })));
        // Its rows are clickable where they are drawn: the second is USD.
        redraw(&mut app, 140, 40);
        let list = app.hit.popup_list;
        click(&mut app, list.x + 3, list.y + 1);
        assert_eq!(app.cfg.currency, "AUD", "one click only highlights");
        click(&mut app, list.x + 3, list.y + 1);
        assert_eq!(app.cfg.currency, "USD");
        assert!(app.popup.is_none());
        // The button is there on the fare-intel screen too.
        app.feed("i");
        redraw(&mut app, 140, 40);
        let button = app.hit.currency;
        click(&mut app, button.x + 1, button.y);
        assert!(matches!(app.popup, Some(Popup::Currency { .. })));
    }

    #[test]
    fn the_settings_button_opens_a_screen_that_shows_and_changes_everything() {
        let mut app = rendered(140, 40, "");
        let button = app.hit.setup;
        assert!(
            button.width >= 10,
            "the settings button is on screen: {button:?}"
        );
        click(&mut app, button.x + 2, button.y);
        assert!(matches!(app.popup, Some(Popup::Settings { sel: 0 })));
        let text = screen_of(&mut app, 140, 40);
        for needle in [
            "SETTINGS",
            "API KEY",
            "NONE - FARES ARE SIMULATED",
            "AUD  AUSTRALIAN DOLLAR",
            "THEME",
            "EACH ITS OWN COLOUR",
            "AS THE CONSOLE HAS IT",
            "6 HOURS",
            "CTRL+V",
            "NOT WRITTEN: THIS IS A SCRIPTED RUN",
            "S SAVE NOW",
        ] {
            assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
        }
        // Rows are clickable where they are drawn; the sixth is BORDERS.
        // One click picks it, a click on the picked row changes it.
        let list = app.hit.popup_list;
        assert_eq!(list.height, 8, "one row a setting");
        click(&mut app, list.x + 4, list.y + 5);
        assert!(matches!(app.popup, Some(Popup::Settings { sel: 5 })));
        assert!(app.cfg.borders, "the first click only picks the row");
        click(&mut app, list.x + 4, list.y + 5);
        assert!(!app.cfg.borders);
        assert!(screen_of(&mut app, 140, 40).contains("HIDDEN"));
        // The first row is the key: picked already, so one click opens its box.
        app.feed("esc");
        app.feed("f2");
        redraw(&mut app, 140, 40);
        let list = app.hit.popup_list;
        click(&mut app, list.x + 4, list.y);
        assert!(matches!(app.popup, Some(Popup::Text { .. })));
        assert!(screen_of(&mut app, 140, 40).contains("PASTE IT WITH CTRL+V OR A RIGHT-CLICK"));
        // The button is on the fare-intel screen too.
        app.feed("esc");
        app.feed("esc");
        app.feed("i");
        redraw(&mut app, 140, 40);
        let button = app.hit.setup;
        click(&mut app, button.x + 2, button.y);
        assert!(matches!(app.popup, Some(Popup::Settings { .. })));
    }

    #[test]
    fn settings_say_where_the_file_is_and_what_the_key_is_doing() {
        let mut app = rendered(140, 40, "f2");
        // As a real (unscripted) run would have it: a file written, a key in use.
        let long = r"D:\Somewhere\With\A\Very\Long\Path\That\Goes\On\And\On\And\On\target\release\icarus-search.toml";
        app.saved = Some((
            std::path::PathBuf::from(long),
            chrono::NaiveTime::from_hms_opt(14, 2, 11).unwrap(),
        ));
        app.live = true;
        app.key_tail = "9f3a".into();
        app.key_state = KeyState::Checking;
        let text = screen_of(&mut app, 140, 40);
        assert!(
            text.contains(r"...") && text.contains(r"\target\release\icarus-search.toml"),
            "the end of a long path is what is kept:\n{text}"
        );
        assert!(text.contains("SAVED AT 14:02:11"), "{text}");
        assert!(text.contains("••••9f3a  CHECKING..."), "{text}");
        assert!(!text.contains("NONE - FARES ARE SIMULATED"));
        app.key_state = KeyState::Accepted;
        app.quota = Some(Quota {
            left: 244,
            per_month: 250,
        });
        assert!(
            screen_of(&mut app, 140, 40).contains("••••9f3a  ACCEPTED. 244 OF 250 SEARCHES LEFT")
        );
        app.key_state = KeyState::Rejected("Invalid API key.".into());
        assert!(screen_of(&mut app, 140, 40).contains("REJECTED BY SERPAPI"));
        app.key_state = KeyState::Unchecked("offline".into());
        assert!(screen_of(&mut app, 140, 40).contains("COULD NOT BE REACHED"));
    }

    #[test]
    fn a_narrow_header_keeps_the_source_and_the_buttons() {
        let top = |app: &mut App, width: u16| {
            screen_of(app, width, 24)
                .lines()
                .next()
                .unwrap()
                .to_string()
        };
        // At the smallest supported width the clock and the strapline give
        // way. What the fares are, and the two buttons, do not.
        let mut app = rendered(96, 24, "");
        let line = top(&mut app, 96);
        for needle in ["ICARUS-SEARCH", "SIMULATED DATA", "F2 SETTINGS", "AUD"] {
            assert!(line.contains(needle), "missing {needle:?} in: {line}");
        }
        assert!(
            app.hit.setup.width > 0 && app.hit.currency.width > 0,
            "and both can still be clicked"
        );
        // Live mode has more to say, and used to lose all of it at 120
        // columns. The fuel gauge stays; the call counter goes first.
        app.live = true;
        app.source = "SERPAPI";
        app.key_state = KeyState::Accepted;
        app.quota = Some(Quota {
            left: 231,
            per_month: 250,
        });
        app.calls = 12;
        let line = top(&mut app, 96);
        for needle in [
            "ICARUS-SEARCH",
            "LIVE SERPAPI",
            "FUEL 231/250",
            "F2 SETTINGS",
            "AUD",
        ] {
            assert!(line.contains(needle), "missing {needle:?} in: {line}");
        }
        assert!(!line.contains("CALLS"), "{line}");
        let line = top(&mut app, 160);
        assert!(
            line.contains("CALLS 12") && line.contains("FLIGHT INTERCEPT TERMINAL"),
            "{line}"
        );
        // A key the service refused is named where the source would be.
        app.key_state = KeyState::Rejected("Invalid API key.".into());
        assert!(top(&mut app, 120).contains("KEY REJECTED"));
    }

    #[test]
    fn the_survey_box_states_its_cost_in_calls_not_samples() {
        let mut app = rendered(140, 40, "from:LHR to:NRT v");
        let text = screen_of(&mut app, 140, 40);
        for needle in [
            "FARE SURVEY",
            "12 MONTHS",
            "365, EVERY ONE PRICED",
            "FREE (SIMULATOR)",
            "NOTHING IS ESTIMATED",
        ] {
            assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
        }
        // On live data the same year is six calls, set against the fuel left.
        app.live = true;
        app.quota = Some(Quota {
            left: 4,
            per_month: 250,
        });
        let text = screen_of(&mut app, 140, 40);
        assert!(text.contains("6 API CALLS  (FUEL LEFT: 4)"), "{text}");
    }

    #[test]
    fn the_trip_type_and_both_ends_of_the_route_are_colour_coded() {
        use ratatui::style::Color;
        let mut app = rendered(180, 45, "from:SYD to:SGN left s wait");
        let mut terminal = Terminal::new(TestBackend::new(180, 45)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let buf = terminal.backend().buffer();
        // Find a word on screen and report the colours of its first cell.
        let find = |word: &str| -> Vec<(Color, Color)> {
            let mut found = Vec::new();
            for y in 0..45u16 {
                let row: Vec<&str> = (0..180u16).map(|x| buf[(x, y)].symbol()).collect();
                let line = row.concat();
                for (at, _) in line.match_indices(word) {
                    let x = line[..at].chars().count() as u16;
                    found.push((buf[(x, y)].fg, buf[(x, y)].bg));
                }
            }
            found
        };
        let th = app.theme;
        // One-way is a solid amber tag in the form, the table title and the dossier.
        let tags = find("ONE-WAY");
        assert!(tags.len() >= 3, "{tags:?}");
        assert!(
            tags.iter()
                .all(|(fg, bg)| *bg == th.trip(false) && *fg == th.bg),
            "{tags:?}"
        );
        // The origin is cyan and the destination pink, wherever they are named.
        assert!(find("SYD SYDNEY").contains(&(th.route(0.0), th.bg)));
        assert!(find("SGN HO CHI").contains(&(th.route(1.0), th.bg)));
        // ...including as solid tags on the map.
        assert!(find("SYD").contains(&(th.bg, th.route(0.0))));
        assert!(find("SGN").contains(&(th.bg, th.route(1.0))));
    }

    #[test]
    fn tiny_windows_ask_to_be_enlarged() {
        assert!(screen(60, 12, "").contains("WINDOW TOO SMALL"));
    }

    #[test]
    fn intel_screen_shows_both_clocks() {
        let text = screen(140, 40, "from:LHR to:NRT v enter wait");
        for needle in [
            "FARE INTEL",
            "WHEN TO FLY",
            "WHEN TO BUY",
            "WHICH MONTH",
            "HOW MUCH NOTICE",
            "MEASURED",
            "ESTIMATED",
        ] {
            assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
        }
    }

    #[test]
    fn fare_intel_explains_itself() {
        // Wide enough for the guide to sit beside the heatmap.
        let wide = screen(180, 50, "from:SYD to:SGN i wait");
        for needle in [
            "HOW TO READ THIS",
            "CHEAPEST",
            "DEAREST",
            "HALF THE DAYS ARE UNDER",
            "YOU ARE HERE",
            "MONTH'S CHEAPEST DAY",
        ] {
            assert!(wide.contains(needle), "missing {needle:?} in:\n{wide}");
        }
        // Too narrow for that: the same guide is one key away.
        let narrow = screen(120, 40, "from:SYD to:SGN i wait");
        assert!(!narrow.contains("HOW TO READ THIS"));
        assert!(
            narrow.contains("? HOW TO READ"),
            "the footer should point to the guide:\n{narrow}"
        );
        let guide = screen(120, 40, "from:SYD to:SGN i wait ?");
        for needle in [
            "HOW TO READ FARE INTEL",
            "WHEN TO FLY",
            "WHEN TO BUY",
            "USUAL PRICE RANGE",
        ] {
            assert!(guide.contains(needle), "missing {needle:?} in:\n{guide}");
        }
        // Every month of a surveyed year has a best day marked on the grid.
        assert!(wide.matches('▼').count() >= 12, "{wide}");
    }

    #[test]
    fn clicking_a_day_on_the_heatmap_moves_the_cursor_whatever_the_tile_width() {
        for width in [130, 180, 220] {
            let mut app = rendered(width, 50, "from:SYD to:SGN i wait");
            let (grid, cell, start) = (
                app.hit.heat_grid,
                app.hit.heat_cell,
                app.hit.heat_start.unwrap(),
            );
            assert!(cell == 2 || cell == 3, "{width}: tiles are {cell} wide");
            // Ten weeks along, on the Thursday row.
            click(&mut app, grid.x + 10 * cell, grid.y + 3);
            let expected = start + chrono::Duration::days(10 * 7 + 3);
            assert_eq!(
                app.intel.as_ref().map(|i| i.cursor),
                Some(expected),
                "{width}"
            );
        }
    }
}
