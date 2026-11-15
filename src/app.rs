//! Application state and input handling. Nothing in here draws: `ui` reads
//! this state each frame, and records where it drew things in `App::hit` so
//! mouse clicks can be mapped back to what they landed on.

use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    thread,
    time::{Duration as StdDuration, Instant},
};

use chrono::{Datelike, Duration, Local, Months, NaiveDate, NaiveTime, Utc};
use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind},
    layout::{Position, Rect},
};

use crate::{
    airports::{self, Airport},
    cache::Cache,
    config::{self, Config, KeySource, Session},
    console, currency,
    farelog::{FareLog, FareMap},
    fmt,
    geo::{Framing, Projection, Side, V3, View, wrap_lon},
    model::{Cabin, Itinerary, PriceInsights, Query, SearchResult, Stage, Stops, TimeWindow},
    provider::{CALENDAR_DAYS, Provider, ProviderError, Quota, demo::Demo, serpapi::SerpApi},
    score::deal_score,
    ui::{
        canvas::{DotCanvas, Marker},
        globe::Scene,
        theme::{Glyphs, Theme},
    },
    worker::{Job, Purpose, Reply, Worker},
};

/// Selectable date flexibility, as `(days, later only)`: the chosen day
/// alone; then that many days either side of it; then that day and that
/// many after it, for "as soon as possible from tomorrow" - where a day
/// earlier than the one named is no use at all.
pub const FLEX_CHOICES: [(u8, bool); 9] = [
    (0, false),
    (1, false),
    (2, false),
    (3, false),
    (5, false),
    (7, false),
    (3, true),
    (7, true),
    (14, true),
];

/// Fare survey presets: `(label, days ahead)`.
///
/// A survey prices every day of its horizon, a price calendar at a time:
/// one call covers two months of departure dates, so even the full year is
/// six calls. Nothing is sampled and nothing between is estimated.
pub const HORIZONS: [(&str, i64); 4] = [
    ("1 MONTH", 30),
    ("3 MONTHS", 91),
    ("6 MONTHS", 182),
    ("12 MONTHS", 364),
];

/// The survey that fills the whole heatmap.
pub const YEAR: usize = HORIZONS.len() - 1;

/// Rows of the settings screen, top to bottom: everything in the settings
/// file that is worth changing without leaving the program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    ApiKey,
    Currency,
    Theme,
    Charset,
    Continents,
    Borders,
    TextSize,
    CacheHours,
}

impl Setting {
    pub const ALL: [Setting; 8] = [
        Setting::ApiKey,
        Setting::Currency,
        Setting::Theme,
        Setting::Charset,
        Setting::Continents,
        Setting::Borders,
        Setting::TextSize,
        Setting::CacheHours,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Setting::ApiKey => "API KEY",
            Setting::Currency => "CURRENCY",
            Setting::Theme => "THEME",
            Setting::Charset => "SYMBOLS",
            Setting::Continents => "CONTINENTS",
            Setting::Borders => "BORDERS",
            Setting::TextSize => "TEXT SIZE",
            Setting::CacheHours => "KEEP RESULTS",
        }
    }
}

/// How long a live search may be reused before it is asked again, in hours.
pub const CACHE_CHOICES: [u32; 6] = [1, 3, 6, 12, 24, 48];

/// What the fare service made of the key in use.
#[derive(Clone, Debug, PartialEq)]
pub enum KeyState {
    /// No key: simulated fares.
    Absent,
    /// Asked, no answer yet.
    Checking,
    Accepted,
    Rejected(String),
    /// The service could not be reached, so the key is neither.
    Unchecked(String),
}

/// What each heading of the TRACKS table sorts by, left to right
/// (the row-number column has no sort).
pub const TRACK_COLUMNS: [Sort; 7] = [
    Sort::Price,
    Sort::Depart,
    Sort::Arrive,
    Sort::Duration,
    Sort::Stops,
    Sort::Carrier,
    Sort::Best,
];

pub struct Options {
    pub key: Option<String>,
    pub demo: bool,
    /// Simulate network delay in demo mode.
    pub latency: bool,
    /// Touch nothing on disk (snapshots, tests).
    pub ephemeral: bool,
    pub charset: Option<String>,
    /// Folder to keep the live source's raw replies in (`--dump`).
    pub dump: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Form,
    Radar,
    Tracks,
    Globe,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Trip,
    From,
    To,
    Depart,
    Return,
    Flex,
    DepWindow,
    ArrWindow,
    Stops,
    Cabin,
    Pax,
    Airlines,
    Launch,
}

pub struct Form {
    pub round_trip: bool,
    pub origin: Option<&'static Airport>,
    pub dest: Option<&'static Airport>,
    pub depart: NaiveDate,
    pub ret: NaiveDate,
    /// Scan this many days either side of `depart`...
    pub flex: u8,
    /// ...or, if set, only that many days after it.
    pub flex_later: bool,
    pub dep_window: TimeWindow,
    pub arr_window: TimeWindow,
    pub stops: Stops,
    pub cabin: Cabin,
    pub adults: u8,
    pub airlines: Vec<String>,
    pub cursor: usize,
}

impl Form {
    fn from_session(s: &Session, today: NaiveDate) -> Self {
        let db = airports::db();
        let depart = s
            .depart
            .filter(|d| *d >= today)
            .unwrap_or(today + Duration::days(30));
        let ret = s
            .ret
            .filter(|r| *r >= depart)
            .unwrap_or(depart + Duration::days(7));
        let (flex, flex_later) = if FLEX_CHOICES.contains(&(s.flex, s.flex_later)) {
            (s.flex, s.flex_later)
        } else {
            (0, false)
        };
        Self {
            round_trip: s.round_trip,
            origin: db.get(&s.origin),
            dest: db.get(&s.dest),
            depart,
            ret,
            flex,
            flex_later,
            dep_window: s.dep_window,
            arr_window: s.arr_window,
            stops: s.stops,
            cabin: s.cabin,
            adults: s.adults.clamp(1, 9),
            airlines: s.airlines.clone(),
            cursor: 0,
        }
    }

    fn to_session(&self) -> Session {
        Session {
            origin: self.origin.map(|a| a.iata.to_string()).unwrap_or_default(),
            dest: self.dest.map(|a| a.iata.to_string()).unwrap_or_default(),
            round_trip: self.round_trip,
            depart: Some(self.depart),
            ret: Some(self.ret),
            flex: self.flex,
            flex_later: self.flex_later,
            cabin: self.cabin,
            stops: self.stops,
            adults: self.adults,
            airlines: self.airlines.clone(),
            dep_window: self.dep_window,
            arr_window: self.arr_window,
        }
    }

    /// Rows shown, top to bottom. One-way trips have no return date.
    pub fn fields(&self) -> Vec<Field> {
        use Field::*;
        [
            Trip, From, To, Depart, Return, Flex, DepWindow, ArrWindow, Stops, Cabin, Pax,
            Airlines, Launch,
        ]
        .into_iter()
        .filter(|f| self.round_trip || *f != Return)
        .collect()
    }

    pub fn field(&self) -> Field {
        let fields = self.fields();
        fields[self.cursor.min(fields.len() - 1)]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endpoint {
    Origin,
    Dest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextKind {
    DepWindow,
    ArrWindow,
    Airlines,
    ApiKey,
}

pub enum Popup {
    Airport {
        target: Endpoint,
        input: String,
        hits: Vec<&'static Airport>,
        sel: usize,
    },
    Calendar {
        ret: bool,
        cursor: NaiveDate,
    },
    Text {
        kind: TextKind,
        input: String,
        error: Option<String>,
    },
    Dossier {
        scroll: u16,
    },
    Survey {
        horizon: usize,
    },
    /// Pick a currency from the list, or type any 3-letter code.
    Currency {
        input: String,
        sel: usize,
    },
    /// The settings file, laid out for changing: `sel` indexes `Setting::ALL`.
    Settings {
        sel: usize,
    },
    Help,
    /// How to read the fare-intel screen.
    Guide,
}

/// The state of one search.
pub enum Slot {
    Pending,
    Ready(SearchResult),
    Failed(ProviderError),
}

impl Slot {
    pub fn lowest(&self) -> Option<f64> {
        match self {
            Slot::Ready(found) => found.lowest_price(),
            _ => None,
        }
    }
}

impl From<Result<SearchResult, ProviderError>> for Slot {
    fn from(result: Result<SearchResult, ProviderError>) -> Self {
        match result {
            Ok(found) => Slot::Ready(found),
            Err(e) => Slot::Failed(e),
        }
    }
}

/// Second half of a round trip: return options for a chosen outbound.
pub struct ReturnStage {
    pub outbound: Itinerary,
    pub slot: Slot,
    pub sel: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    Price,
    Best,
    Duration,
    Depart,
    Arrive,
    Stops,
    Carrier,
}

impl Sort {
    /// In number-key order: `1` is price, `2` best value, and so on.
    pub const ALL: [Sort; 7] = [
        Sort::Price,
        Sort::Best,
        Sort::Duration,
        Sort::Depart,
        Sort::Arrive,
        Sort::Stops,
        Sort::Carrier,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Sort::Price => "PRICE",
            Sort::Best => "BEST VALUE",
            Sort::Duration => "DURATION",
            Sort::Depart => "DEPARTURE",
            Sort::Arrive => "ARRIVAL",
            Sort::Stops => "STOPS",
            Sort::Carrier => "CARRIER",
        }
    }
}

/// Results of the current scan, one slot per departure date.
pub struct Board {
    pub slots: BTreeMap<NaiveDate, Slot>,
    /// Which date the TRACKS table is showing.
    pub active: NaiveDate,
    pub sel: usize,
    pub offset: usize,
    pub sort: Sort,
    /// Largest first. Choosing the same sort twice flips this.
    pub descending: bool,
    pub ret: Option<ReturnStage>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Good,
    Warn,
    Alert,
}

/// The full-screen fare intelligence view.
pub struct Intel {
    pub cursor: NaiveDate,
}

/// Screen regions recorded by the last draw, for mouse hit-testing.
#[derive(Clone, Copy, Debug, Default)]
pub struct Hit {
    pub form: Rect,
    pub form_rows: Rect,
    pub radar: Rect,
    pub radar_rows: Rect,
    pub radar_offset: usize,
    pub globe: Rect,
    pub globe_inner: Rect,
    pub tracks: Rect,
    pub track_rows: Rect,
    /// The TRACKS heading row, and the columns of each sortable heading
    /// as `(first, one past last)`, in `TRACK_COLUMNS` order.
    pub track_head: Rect,
    pub track_cols: [(u16, u16); 7],
    /// The currency shown in the header bar.
    pub currency: Rect,
    /// The settings button in the header bar.
    pub setup: Rect,
    pub popup: Rect,
    pub popup_list: Rect,
    /// Index of the first row shown in `popup_list`.
    pub popup_list_offset: usize,
    /// Day grid of the calendar popup: 7 columns of 4 cells.
    pub cal_grid: Rect,
    /// Day grid of the INTEL heatmap: `heat_cell` cells per week, one row
    /// per weekday.
    pub heat_grid: Rect,
    pub heat_cell: u16,
    pub heat_start: Option<NaiveDate>,
}

/// What the globe should draw besides geography.
#[derive(Default)]
pub struct Routes {
    /// Legs of every other itinerary on the board.
    pub faint: Vec<(V3, V3)>,
    /// The selected itinerary.
    pub primary: Vec<Leg>,
    /// The chosen outbound, while picking a return.
    pub secondary: Vec<Leg>,
    /// `true` when `primary` is only the straight line between the chosen
    /// airports, before any search has run.
    pub planned: bool,
    pub marks: Vec<Mark>,
}

/// One flight of a journey. `t0` and `t1` say where its two ends fall on
/// the journey's colour ramp: 0.0 is the trip's origin, 1.0 its destination,
/// and each stop is one step further along. A return flight runs 1.0 to 0.0,
/// so a place keeps its colour whichever way you are flying.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Leg {
    pub a: V3,
    pub b: V3,
    pub t0: f64,
    pub t1: f64,
}

pub struct Mark {
    pub at: V3,
    pub code: &'static str,
    pub kind: MarkKind,
    /// Position on the journey's colour ramp; see [`Leg`].
    pub tone: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkKind {
    Origin,
    Dest,
    Via,
}

struct Drag {
    last: (u16, u16),
    moved: bool,
}

pub struct App {
    pub cfg: Config,
    pub theme: Theme,
    pub glyphs: Glyphs,
    pub marker: Marker,
    pub form: Form,
    pub focus: Pane,
    pub popup: Option<Popup>,
    pub board: Board,
    /// The query behind the board's results (its `depart` is the scan centre).
    pub searched: Option<Query>,
    pub view: View,
    /// Where the view is gliding to.
    pub goal: Option<Framing>,
    /// The view was last set by auto-framing (not by hand), so it should be
    /// re-framed if the pane changes size.
    framed: bool,
    framed_for: (u16, u16),
    /// Idle rotation, until the user takes hold of the globe or picks a route.
    pub spin: bool,
    pub globe_full: bool,
    pub hover: Option<&'static Airport>,
    /// How many other airports share the hovered spot on screen.
    pub hover_more: usize,
    pub hit: Hit,
    /// The planet as last drawn, and the view it was drawn for; see
    /// `ui::globe`. Redrawn only when that view changes.
    pub geography: Option<(Scene, DotCanvas)>,
    pub status: (Level, String),
    pub quota: Option<Quota>,
    /// Live API calls made this session.
    pub calls: u32,
    pub inflight: usize,
    /// Calendar calls of a fare survey still to come back.
    pub survey_left: usize,
    /// Days the survey in progress has priced so far.
    survey_found: usize,
    pub tick: u64,
    /// Animation time in seconds. Everything that moves is driven by this,
    /// not by frame count, so the frame rate can change without the
    /// animation changing speed.
    pub clock: f64,
    pub live: bool,
    pub source: &'static str,
    pub key_source: Option<KeySource>,
    pub key_state: KeyState,
    /// The last four characters of the key in use: enough to tell two keys
    /// apart, no use to anyone reading over a shoulder. Empty with no key.
    pub key_tail: String,
    /// Where the settings file was last written, and when.
    pub saved: Option<(PathBuf, NaiveTime)>,
    /// Reads the system clipboard, for Ctrl+V where the terminal does not
    /// paste by itself. A field so that tests can supply their own.
    pub clipboard: fn() -> Option<String>,
    /// The settings row to go back to when a picker opened from the
    /// settings screen closes.
    settings_return: Option<usize>,
    dump: Option<PathBuf>,
    pub quit: bool,
    pub fares: FareLog,
    /// Price insights by (route, departure date), from this session's searches.
    pub insights: HashMap<(String, NaiveDate), PriceInsights>,
    pub intel: Option<Intel>,
    /// The route a live year-survey was last offered for, so opening INTEL
    /// again does not ask twice.
    survey_offered: Option<String>,
    drag: Option<Drag>,
    worker: Worker,
    generation: u64,
    ephemeral: bool,
    latency: bool,
    /// `clock` at the most recent key press or click.
    last_input: f64,
    last_click: Option<(Instant, Position)>,
}

impl App {
    pub fn new(cfg: Config, session: Session, opts: Options) -> Self {
        let today = Local::now().date_naive();
        let resolved = if opts.demo {
            None
        } else {
            config::resolve_key(opts.key.as_deref(), &cfg)
        };
        let key_tail = resolved
            .as_ref()
            .map_or(String::new(), |(key, _)| tail(key));
        let (provider, live, source, key_source): (Box<dyn Provider>, bool, &'static str, _) =
            match resolved {
                Some((key, from)) => (
                    Box::new(SerpApi::new(key, opts.dump.clone())),
                    true,
                    "SERPAPI",
                    Some(from),
                ),
                None => (Box::new(Demo::new(opts.latency)), false, "SIMULATOR", None),
            };
        let paths = if opts.ephemeral {
            None
        } else {
            config::paths()
        };
        let cache = paths
            .as_ref()
            .and_then(|p| Cache::open(p.cache.clone(), cfg.cache_hours));
        let fares = match &paths {
            Some(p) => FareLog::open(p.fares.clone()),
            None => FareLog::in_memory(),
        };
        let (glyphs, marker) = Glyphs::detect(opts.charset.as_deref().unwrap_or(&cfg.charset));
        let form = Form::from_session(&session, today);
        let view = View {
            lat0: 20.0,
            lon0: form.origin.map_or(0.0, |a| a.pos.lon),
            zoom: 1.0,
            proj: Projection::Globe,
            drop: 0.0,
            shift: 0.0,
        };
        let status = if live {
            (
                Level::Good,
                "LIVE FARES ARMED. SET A ROUTE, THEN PRESS S TO SCAN.".to_string(),
            )
        } else {
            (
                Level::Warn,
                "SIMULATED FARES - NOT REAL PRICES. PRESS F2 TO ADD AN API KEY FOR LIVE ONES."
                    .to_string(),
            )
        };
        let mut app = Self {
            theme: Theme::by_name(&cfg.theme),
            cfg,
            glyphs,
            marker,
            spin: form.origin.is_none() && form.dest.is_none(),
            board: Board {
                slots: BTreeMap::new(),
                active: form.depart,
                sel: 0,
                offset: 0,
                sort: Sort::Price,
                descending: false,
                ret: None,
            },
            form,
            focus: Pane::Form,
            popup: None,
            searched: None,
            view,
            goal: None,
            framed: true,
            framed_for: (0, 0),
            globe_full: false,
            hover: None,
            hover_more: 0,
            hit: Hit::default(),
            geography: None,
            status,
            quota: None,
            calls: 0,
            inflight: 0,
            survey_left: 0,
            survey_found: 0,
            tick: 0,
            clock: 0.0,
            live,
            source,
            key_source,
            key_state: if live {
                KeyState::Checking
            } else {
                KeyState::Absent
            },
            key_tail,
            saved: None,
            clipboard: console::clipboard_text,
            settings_return: None,
            dump: opts.dump,
            quit: false,
            fares,
            insights: HashMap::new(),
            intel: None,
            survey_offered: None,
            drag: None,
            worker: Worker::spawn(provider, cache),
            generation: 0,
            ephemeral: opts.ephemeral,
            latency: opts.latency,
            last_input: 0.0,
            last_click: None,
        };
        if live {
            app.worker.submit(Job::Quota);
        }
        // Start already facing the remembered route; the first frame's
        // real pane size then refines the framing.
        if let Some(f) = app.route_fit() {
            (app.view.lat0, app.view.lon0, app.view.zoom) = (f.lat0, f.lon0, f.zoom);
            app.view.clamp();
        }
        app
    }

    pub fn set_status(&mut self, level: Level, text: impl Into<String>) {
        self.status = (level, text.into());
    }

    /// True while something on screen is moving and deserves a fast frame rate.
    pub fn busy(&self) -> bool {
        self.inflight > 0
            || self.survey_left > 0
            || self.goal.is_some()
            || self.clock - self.last_input < 3.0
    }

    /// Which of `count` animation frames to show, advancing `per_sec` a second.
    pub fn frame(&self, per_sec: f64, count: usize) -> usize {
        (self.clock * per_sec) as usize % count.max(1)
    }

    // ---------------------------------------------------------------- queries

    /// The form's trip, departing on `depart` (the return date shifts with it).
    pub fn form_query(&self, depart: NaiveDate) -> Option<Query> {
        let (origin, dest) = (self.form.origin?, self.form.dest?);
        Some(Query {
            origin: origin.iata.to_string(),
            dest: dest.iata.to_string(),
            depart,
            ret: self
                .form
                .round_trip
                .then(|| depart + (self.form.ret - self.form.depart)),
            adults: self.form.adults,
            cabin: self.form.cabin,
            stops: self.form.stops,
            airlines: self.form.airlines.clone(),
            dep_window: self.form.dep_window,
            arr_window: self.form.arr_window,
            currency: self.cfg.currency.clone(),
        })
    }

    /// The searched trip, moved to the date the board is showing.
    pub fn active_query(&self) -> Option<Query> {
        let mut q = self.searched.clone()?;
        let shift = self.board.active - q.depart;
        q.depart = self.board.active;
        q.ret = q.ret.map(|r| r + shift);
        Some(q)
    }

    /// Fare-log key for the trip currently in the form.
    pub fn route_key(&self) -> Option<String> {
        self.form_query(self.form.depart)
            .map(|q| q.route_key(self.source))
    }

    pub fn fare_map(&self) -> Option<FareMap> {
        self.route_key()
            .map(|key| FareMap::new(self.fares.latest(&key)))
    }

    /// Departure dates a scan would search, the chosen date first.
    pub fn scan_dates(&self) -> Vec<NaiveDate> {
        let today = Local::now().date_naive();
        let n = i64::from(self.form.flex);
        let earliest = if self.form.flex_later { 0 } else { -n };
        let mut dates: Vec<NaiveDate> = (earliest..=n)
            .map(|k| self.form.depart + Duration::days(k))
            .filter(|d| *d >= today)
            .collect();
        dates.sort_by_key(|d| (*d - self.form.depart).num_days().abs());
        dates
    }

    /// True when the form no longer matches what the board shows.
    pub fn stale(&self) -> bool {
        match (&self.searched, self.form_query(self.form.depart)) {
            (Some(searched), Some(current)) => *searched != current,
            _ => false,
        }
    }

    // ------------------------------------------------------------------ scans

    pub fn launch(&mut self) {
        let today = Local::now().date_naive();
        let Some(query) = self.form_query(self.form.depart) else {
            self.set_status(Level::Alert, "SET BOTH FROM AND TO BEFORE SCANNING.");
            self.focus = Pane::Form;
            self.form.cursor = if self.form.origin.is_none() { 1 } else { 2 };
            return;
        };
        if query.origin == query.dest {
            self.set_status(Level::Alert, "FROM AND TO ARE THE SAME AIRPORT.");
            return;
        }
        if self.form.depart < today {
            self.set_status(Level::Alert, "DEPARTURE DATE IS IN THE PAST.");
            return;
        }
        let dates = self.scan_dates();
        self.generation = self.worker.next_generation();
        self.survey_left = 0; // queued survey jobs died with the old generation
        self.board.slots.clear();
        self.board.active = self.form.depart;
        self.board.sel = 0;
        self.board.offset = 0;
        self.board.ret = None;
        for date in &dates {
            self.board.slots.insert(*date, Slot::Pending);
            if let Some(q) = self.form_query(*date) {
                self.worker.submit(Job::Search {
                    generation: self.generation,
                    purpose: Purpose::Date(*date),
                    query: q,
                    stage: Stage::Outbound,
                });
            }
        }
        self.worker.submit(Job::Quota);
        self.inflight = dates.len();
        self.set_status(
            Level::Info,
            format!(
                "SCANNING {} > {} ACROSS {} DATE(S)",
                query.origin,
                query.dest,
                dates.len()
            ),
        );
        self.searched = Some(query);
        self.focus = Pane::Tracks;
        self.intel = None;
        self.spin = false;
        self.refit();
        self.persist();
    }

    /// Price every day of the coming months to fill the INTEL heatmap.
    pub fn launch_survey(&mut self, horizon: usize) {
        let Some(query) = self.form_query(self.form.depart) else {
            self.set_status(Level::Alert, "SET BOTH FROM AND TO BEFORE SURVEYING.");
            return;
        };
        if query.origin == query.dest {
            self.set_status(Level::Alert, "FROM AND TO ARE THE SAME AIRPORT.");
            return;
        }
        let ranges = self.survey_ranges(horizon);
        if ranges.is_empty() {
            self.set_status(Level::Good, "SURVEY ALREADY UP TO DATE FOR THIS HORIZON.");
            return;
        }
        for (from, to) in &ranges {
            self.worker.submit(Job::Calendar {
                generation: self.generation,
                query: query.clone(),
                from: *from,
                to: *to,
            });
        }
        self.worker.submit(Job::Quota);
        if self.survey_left == 0 {
            self.survey_found = 0;
        }
        self.survey_left += ranges.len();
        let days: i64 = ranges
            .iter()
            .map(|(from, to)| (*to - *from).num_days() + 1)
            .sum();
        let calls = ranges.len();
        self.set_status(
            Level::Info,
            format!(
                "SURVEYING {}: ALL {days} DAYS, IN {calls} CALL{}",
                HORIZONS[horizon.min(YEAR)].0,
                if calls == 1 { "" } else { "S" }
            ),
        );
    }

    /// What a survey of `horizon` would ask for: inclusive date ranges of
    /// at most `CALENDAR_DAYS`, one call each, from today onwards. A range
    /// priced within the last day is left out.
    pub fn survey_ranges(&self, horizon: usize) -> Vec<(NaiveDate, NaiveDate)> {
        let today = Local::now().date_naive();
        let end = today + Duration::days(HORIZONS[horizon.min(YEAR)].1);
        let fresh = Utc::now().timestamp() - 86_400;
        let known = self
            .route_key()
            .map(|k| self.fares.latest(&k))
            .unwrap_or_default();
        let mut ranges = Vec::new();
        let mut from = today;
        while from <= end {
            let to = (from + Duration::days(CALENDAR_DAYS - 1)).min(end);
            let span = (to - from).num_days() + 1;
            let priced = known
                .range(from..=to)
                .filter(|(_, s)| s.seen >= fresh)
                .count() as i64;
            // "Most of its days", not all of them: a day with no flights
            // never gets a price, and must not make the range look stale
            // for ever. A scan's handful of dates falls well short.
            if priced * 2 < span {
                ranges.push((from, to));
            }
            from = to + Duration::days(1);
        }
        ranges
    }

    /// Apply everything the worker has finished since the last frame.
    pub fn drain(&mut self) {
        while let Ok(reply) = self.worker.replies.try_recv() {
            match reply {
                Reply::Quota(answer) => self.on_quota(answer),
                Reply::Search {
                    generation,
                    purpose,
                    route,
                    result,
                    cached,
                } => {
                    self.on_result(generation, purpose, route, result, cached);
                }
                Reply::Calendar {
                    generation,
                    route,
                    result,
                } => self.on_calendar(generation, route, result),
            }
        }
    }

    /// The account's answer: fuel left and, with it, whether the key is good.
    fn on_quota(&mut self, answer: Result<Option<Quota>, ProviderError>) {
        if !self.live {
            return; // an answer about a key that has since been removed
        }
        // Speak up only when someone is waiting to hear: at start-up, after
        // a key is entered, or when a check was asked for. The routine
        // look at the fuel gauge after each scan stays quiet.
        let asked = self.key_state == KeyState::Checking;
        match answer {
            Ok(quota) => {
                if quota.is_some() {
                    self.quota = quota;
                }
                self.key_state = KeyState::Accepted;
                if asked {
                    let fuel = quota.map_or(String::new(), |q| {
                        format!(" {} OF {} SEARCHES LEFT THIS MONTH.", q.left, q.per_month)
                    });
                    self.set_status(
                        Level::Good,
                        format!("KEY ACCEPTED. LIVE FARES ARMED.{fuel}"),
                    );
                }
            }
            Err(ProviderError::Auth(message)) => {
                self.key_state = KeyState::Rejected(message);
                self.set_status(
                    Level::Alert,
                    "API KEY REJECTED. PRESS F2 TO ENTER IT AGAIN.",
                );
            }
            Err(e) if asked => {
                self.set_status(Level::Warn, format!("COULD NOT CHECK THE KEY. {e}"));
                self.key_state = KeyState::Unchecked(e.to_string());
            }
            Err(_) => {}
        }
    }

    /// One piece of a fare survey: two months of days, each with its fare.
    fn on_calendar(
        &mut self,
        generation: u64,
        route: String,
        result: Result<Vec<(NaiveDate, f64)>, ProviderError>,
    ) {
        if generation != self.generation {
            return; // the scan this belonged to has been replaced
        }
        if self.live {
            self.calls += 1;
        }
        self.survey_left = self.survey_left.saturating_sub(1);
        match result {
            Ok(fares) => {
                self.survey_found += fares.len();
                self.fares
                    .record_all(&route, &fares, Utc::now().timestamp());
                if self.survey_left > 0 {
                    return;
                }
                if self.survey_found == 0 {
                    self.set_status(Level::Warn, "SURVEY FOUND NO FARES ON THIS ROUTE.");
                } else {
                    self.set_status(
                        Level::Good,
                        format!("SURVEY COMPLETE: {} DAYS PRICED.", self.survey_found),
                    );
                }
            }
            // Every failure is worth saying, not only the fatal ones: a
            // piece that failed is two months missing from the heatmap.
            Err(e) => self.set_status(Level::Alert, e.to_string()),
        }
    }

    fn on_result(
        &mut self,
        generation: u64,
        purpose: Purpose,
        route: String,
        result: Result<SearchResult, ProviderError>,
        cached: bool,
    ) {
        if generation != self.generation {
            return; // answer to a scan the user has already replaced
        }
        if self.live && !cached {
            self.calls += 1;
        }
        if let (Ok(found), Purpose::Date(date)) = (&result, &purpose) {
            let seen = Utc::now().timestamp() - i64::from(found.cache_age_min.unwrap_or(0)) * 60;
            if let Some(price) = found.lowest_price() {
                self.fares.record(&route, *date, price, seen);
            }
            if let Some(insights) = &found.insights {
                self.insights
                    .insert((route.clone(), *date), insights.clone());
            }
        }
        let fatal = matches!(
            &result,
            Err(ProviderError::Auth(_) | ProviderError::Quota(_))
        );
        match purpose {
            Purpose::Date(date) => {
                self.inflight = self.inflight.saturating_sub(1);
                let active = date == self.board.active;
                if let Err(e) = &result {
                    if active || fatal {
                        self.set_status(Level::Alert, e.to_string());
                    }
                }
                self.board.slots.insert(date, Slot::from(result));
                if active {
                    self.board.sel = 0;
                    self.board.offset = 0;
                    self.refit();
                }
                if self.inflight == 0 && !fatal {
                    self.summarize_scan();
                }
            }
            Purpose::Return => {
                self.inflight = self.inflight.saturating_sub(1);
                match &result {
                    Err(e) => self.set_status(Level::Alert, e.to_string()),
                    Ok(found) => self.set_status(
                        Level::Good,
                        format!(
                            "{} RETURN OPTION(S). ENTER FOR THE FULL DOSSIER.",
                            found.itineraries.len()
                        ),
                    ),
                }
                if let Some(stage) = &mut self.board.ret {
                    stage.slot = Slot::from(result);
                    stage.sel = 0;
                }
                self.board.offset = 0;
            }
        }
    }

    fn summarize_scan(&mut self) {
        let best = self
            .board
            .slots
            .iter()
            .filter_map(|(date, slot)| slot.lowest().map(|price| (price, *date)))
            .min_by(|a, b| a.0.total_cmp(&b.0));
        // In a hurry, the first day with a seat matters as much as the
        // cheapest: name both when they are different days.
        let soonest = self
            .board
            .slots
            .iter()
            .find_map(|(date, slot)| slot.lowest().map(|price| (price, *date)));
        match best {
            Some((price, date)) => {
                let cur = &self.cfg.currency;
                let mut text = format!(
                    "SCAN COMPLETE. CHEAPEST: {} AT {}",
                    fmt::day_short(date),
                    fmt::money(price, cur)
                );
                if let Some((first_price, first)) = soonest.filter(|(_, first)| *first != date) {
                    text.push_str(&format!(
                        ".  SOONEST: {} AT {}",
                        fmt::day_short(first),
                        fmt::money(first_price, cur)
                    ));
                }
                self.set_status(Level::Good, text);
            }
            None if self
                .board
                .slots
                .values()
                .all(|s| matches!(s, Slot::Failed(_))) => {}
            None => self.set_status(
                Level::Warn,
                "SCAN COMPLETE. NO FLIGHTS MATCH THESE PARAMETERS.",
            ),
        }
    }

    // ----------------------------------------------------------------- tracks

    /// The slot the TRACKS table is showing.
    pub fn slot(&self) -> Option<&Slot> {
        match &self.board.ret {
            Some(stage) => Some(&stage.slot),
            None => self.board.slots.get(&self.board.active),
        }
    }

    /// Itineraries on show, in the current sort order.
    pub fn tracks(&self) -> Vec<&Itinerary> {
        let Some(Slot::Ready(found)) = self.slot() else {
            return Vec::new();
        };
        let mut list: Vec<&Itinerary> = found.itineraries.iter().collect();
        let price = |it: &Itinerary| it.price.unwrap_or(f64::MAX);
        let by_price = |a: &&Itinerary, b: &&Itinerary| price(a).total_cmp(&price(b));
        let rate = self.rate();
        match self.board.sort {
            Sort::Price => list.sort_by(|a, b| by_price(a, b).then(a.total_min.cmp(&b.total_min))),
            Sort::Best => list.sort_by(|a, b| deal_score(a, rate).total_cmp(&deal_score(b, rate))),
            Sort::Duration => {
                list.sort_by(|a, b| a.total_min.cmp(&b.total_min).then(by_price(a, b)))
            }
            Sort::Depart => list.sort_by(|a, b| a.dep().cmp(&b.dep()).then(by_price(a, b))),
            Sort::Arrive => list.sort_by(|a, b| a.arr().cmp(&b.arr()).then(by_price(a, b))),
            Sort::Stops => list.sort_by(|a, b| a.stops().cmp(&b.stops()).then(by_price(a, b))),
            Sort::Carrier => list.sort_by_cached_key(|it| it.carriers().join("+").to_uppercase()),
        }
        if self.board.descending {
            list.reverse();
        }
        list
    }

    /// The fare currency's rough rate to the dollar, for `deal_score`.
    pub fn rate(&self) -> f64 {
        currency::per_usd(&self.cfg.currency)
    }

    /// Sort the tracks. Asking for the sort already in force reverses it.
    pub fn set_sort(&mut self, sort: Sort) {
        if self.board.sort == sort {
            self.board.descending = !self.board.descending;
        } else {
            self.board.sort = sort;
            self.board.descending = false;
        }
        self.set_sel(0);
        self.board.offset = 0;
        let order = if self.board.descending {
            "HIGH TO LOW"
        } else {
            "LOW TO HIGH"
        };
        self.set_status(
            Level::Info,
            format!("SORTED BY {}, {order}. AGAIN TO REVERSE.", sort.label()),
        );
    }

    pub fn sel(&self) -> usize {
        self.board
            .ret
            .as_ref()
            .map_or(self.board.sel, |stage| stage.sel)
    }

    fn set_sel(&mut self, index: usize) {
        match &mut self.board.ret {
            Some(stage) => stage.sel = index,
            None => self.board.sel = index,
        }
    }

    pub fn selected(&self) -> Option<&Itinerary> {
        self.tracks().get(self.sel()).copied()
    }

    fn move_sel(&mut self, delta: i64) {
        let n = self.tracks().len();
        if n > 0 {
            let next = (self.sel() as i64 + delta).clamp(0, n as i64 - 1) as usize;
            self.set_sel(next);
        }
    }

    /// Enter on a track: on a round trip's outbound list this fetches the
    /// return flights; otherwise it opens the full dossier.
    fn activate_track(&mut self) {
        let Some(chosen) = self.selected().cloned() else {
            return;
        };
        let awaiting_return =
            self.board.ret.is_none() && self.searched.as_ref().is_some_and(|q| q.ret.is_some());
        if awaiting_return {
            if let (Some(token), Some(query)) = (chosen.token.clone(), self.active_query()) {
                self.board.ret = Some(ReturnStage {
                    outbound: chosen,
                    slot: Slot::Pending,
                    sel: 0,
                });
                self.board.offset = 0;
                self.inflight += 1;
                self.worker.submit(Job::Search {
                    generation: self.generation,
                    purpose: Purpose::Return,
                    query,
                    stage: Stage::Return { token },
                });
                self.worker.submit(Job::Quota);
                self.set_status(Level::Info, "OUTBOUND LOCKED. ACQUIRING RETURN OPTIONS...");
                return;
            }
        }
        self.popup = Some(Popup::Dossier { scroll: 0 });
    }

    fn leave_return_stage(&mut self) {
        if self.board.ret.take().is_some() {
            self.board.offset = 0;
            self.set_status(Level::Info, "BACK TO OUTBOUND OPTIONS.");
        }
    }

    /// Move the board to the previous or next scanned date.
    fn step_date(&mut self, delta: i64) {
        let dates: Vec<NaiveDate> = self.board.slots.keys().copied().collect();
        let Some(at) = dates.iter().position(|d| *d == self.board.active) else {
            return;
        };
        let next = (at as i64 + delta).clamp(0, dates.len() as i64 - 1) as usize;
        self.show_date(dates[next]);
    }

    fn show_date(&mut self, date: NaiveDate) {
        if date != self.board.active && self.board.slots.contains_key(&date) {
            self.board.active = date;
            self.board.sel = 0;
            self.board.offset = 0;
            self.board.ret = None;
        }
    }

    pub fn open_booking(&mut self) {
        let url = match self.slot() {
            Some(Slot::Ready(found)) => found.browser_url.clone(),
            _ => None,
        }
        .or_else(|| self.active_query().map(|q| q.browser_url()))
        .or_else(|| self.form_query(self.form.depart).map(|q| q.browser_url()));
        let Some(url) = url else {
            self.set_status(Level::Warn, "NOTHING TO OPEN YET. SET A ROUTE FIRST.");
            return;
        };
        if self.ephemeral {
            self.set_status(Level::Info, format!("WOULD OPEN {url}"));
        } else {
            match open::that(&url) {
                Ok(()) => {
                    self.set_status(Level::Good, "OPENED THIS SEARCH IN YOUR BROWSER TO BOOK.")
                }
                Err(e) => self.set_status(Level::Alert, format!("COULD NOT OPEN A BROWSER: {e}")),
            }
        }
    }

    // ------------------------------------------------------------------ globe

    /// Flight paths and airport markers for the globe.
    pub fn routes(&self) -> Routes {
        let db = airports::db();
        // Each flight is one step along the colour ramp. A return flight
        // counts down instead of up, so a place keeps the same colour
        // whichever way it is being flown through.
        let tone = |step: usize, of: usize, back: bool| {
            let t = step as f64 / of.max(1) as f64;
            if back { 1.0 - t } else { t }
        };
        let legs = |it: &Itinerary, back: bool| -> Vec<Leg> {
            let n = it.segments.len();
            it.segments
                .iter()
                .enumerate()
                .filter_map(|(i, s)| {
                    Some(Leg {
                        a: db.get(&s.from)?.vec,
                        b: db.get(&s.to)?.vec,
                        t0: tone(i, n, back),
                        t1: tone(i + 1, n, back),
                    })
                })
                .collect()
        };
        let mut routes = Routes::default();
        let mut vias: Vec<(&'static Airport, f64)> = Vec::new();
        let mut note_vias = |it: &Itinerary, back: bool| {
            for (i, layover) in it.layovers.iter().enumerate() {
                if let Some(airport) = db.get(&layover.airport) {
                    if !vias.iter().any(|(v, _)| v.iata == airport.iata) {
                        vias.push((airport, tone(i + 1, it.segments.len(), back)));
                    }
                }
            }
        };
        let selected = self.selected();
        // While a return is being chosen, the track in hand flies home.
        let returning = self.board.ret.is_some();
        if let Some(it) = selected {
            routes.primary = legs(it, returning);
            note_vias(it, returning);
        }
        if let Some(stage) = &self.board.ret {
            routes.secondary = legs(&stage.outbound, false);
            note_vias(&stage.outbound, false);
        }
        // Everything else on the board, de-duplicated, as background tracks.
        let mut seen: Vec<(String, String)> = Vec::new();
        for it in self.tracks().into_iter().take(40) {
            if selected.is_some_and(|s| std::ptr::eq(s, it)) {
                continue;
            }
            for s in &it.segments {
                let pair = (s.from.clone(), s.to.clone());
                if !seen.contains(&pair) {
                    if let (Some(a), Some(b)) = (db.get(&s.from), db.get(&s.to)) {
                        routes.faint.push((a.vec, b.vec));
                    }
                    seen.push(pair);
                }
            }
        }
        if routes.primary.is_empty() && routes.secondary.is_empty() {
            if let (Some(a), Some(b)) = (self.form.origin, self.form.dest) {
                routes.primary.push(Leg {
                    a: a.vec,
                    b: b.vec,
                    t0: 0.0,
                    t1: 1.0,
                });
                routes.planned = true;
            }
        }
        for (via, tone) in vias {
            routes.marks.push(Mark {
                at: via.vec,
                code: via.iata,
                kind: MarkKind::Via,
                tone,
            });
        }
        if let Some(a) = self.form.origin {
            routes.marks.push(Mark {
                at: a.vec,
                code: a.iata,
                kind: MarkKind::Origin,
                tone: 0.0,
            });
        }
        if let Some(b) = self.form.dest {
            routes.marks.push(Mark {
                at: b.vec,
                code: b.iata,
                kind: MarkKind::Dest,
                tone: 1.0,
            });
        }
        routes
    }

    /// View that frames the chosen airports and the selected itinerary.
    fn route_fit(&self) -> Option<Framing> {
        let routes = self.routes();
        let legs: Vec<(V3, V3)> = routes
            .primary
            .iter()
            .chain(&routes.secondary)
            .map(|leg| (leg.a, leg.b))
            .collect();
        let points: Vec<V3> = [self.form.origin, self.form.dest]
            .into_iter()
            .flatten()
            .map(|a| a.vec)
            .collect();
        let (w, h) = self.globe_dots();
        self.view.fit(&legs, &points, w, h)
    }

    /// Glide the view to frame the current route.
    pub fn refit(&mut self) {
        if let Some(goal) = self.route_fit() {
            self.goal = Some(goal);
            self.spin = false;
            self.framed = true;
            self.framed_for = (self.hit.globe_inner.width, self.hit.globe_inner.height);
        }
    }

    /// The user has taken the globe in hand: stop steering it for them.
    fn release_view(&mut self) {
        self.goal = None;
        self.spin = false;
        self.framed = false;
    }

    /// Canvas size of the globe pane in dots. Before the first frame is
    /// drawn there is no pane yet, so assume a typical one.
    fn globe_dots(&self) -> (f64, f64) {
        let inner = self.hit.globe_inner;
        if inner.width < 8 || inner.height < 4 {
            return (164.0, 56.0);
        }
        (f64::from(inner.width) * 2.0, f64::from(inner.height) * 4.0)
    }

    /// Centre of a terminal cell, in globe-canvas dots.
    fn cell_to_dots(&self, pos: Position) -> (f64, f64) {
        let inner = self.hit.globe_inner;
        (
            f64::from(pos.x.saturating_sub(inner.x)) * 2.0 + 1.0,
            f64::from(pos.y.saturating_sub(inner.y)) * 4.0 + 2.0,
        )
    }

    /// The airports drawn at a mouse position, likeliest first. Smaller
    /// airports become eligible as you zoom in, matching what is drawn.
    ///
    /// The test is made on screen, not on the ground: "which airports are
    /// drawn in the cell that was clicked?". Measuring in degrees instead
    /// goes wrong toward the edge of the globe, where foreshortening
    /// squeezes ten or twenty degrees of planet into a single character.
    fn airports_at(&self, pos: Position) -> Vec<&'static Airport> {
        if !self.hit.globe_inner.contains(pos) {
            return Vec::new();
        }
        let zoom = self.view.zoom;
        let max_size = if zoom >= 6.0 {
            2
        } else if zoom >= 2.5 {
            1
        } else {
            0
        };
        let (w, h) = self.globe_dots();
        let (cx, cy) = self.cell_to_dots(pos);
        let mut found: Vec<(f64, &'static Airport)> = airports::db()
            .all()
            .iter()
            .filter(|a| a.size <= max_size)
            .filter_map(|a| {
                let (x, y, side) = self.view.project(a.vec, w, h);
                let (dx, dy) = ((x - cx).abs(), (y - cy).abs());
                // A cell is 2 x 4 dots; allow one dot of slack all round.
                let hit = side == Side::Front && dx <= 2.0 && dy <= 3.0;
                // Bigger airports win ties with small fields beside them.
                hit.then(|| {
                    (
                        dx.hypot(dy) * [1.0, 1.6, 2.5][usize::from(a.size.min(2))],
                        a,
                    )
                })
            })
            .collect();
        found.sort_by(|a, b| a.0.total_cmp(&b.0));
        found.into_iter().map(|(_, a)| a).collect()
    }

    fn pick_at(&mut self, pos: Position, wanted: Endpoint) {
        // The first click always sets the origin: a destination alone is no use.
        let target = if self.form.origin.is_none() {
            Endpoint::Origin
        } else {
            wanted
        };
        let mut found = self.airports_at(pos);
        match found.len() {
            0 => self.set_status(Level::Warn, "NO AIRPORT THERE. ZOOM IN FOR SMALLER FIELDS."),
            1 => self.set_endpoint(target, found[0]),
            _ => {
                // Several airports share this spot on screen (all of
                // London's fit in one character). Guessing would quietly
                // choose wrongly, so offer the candidates instead.
                found.truncate(40);
                self.popup = Some(Popup::Airport {
                    target,
                    input: String::new(),
                    hits: found,
                    sel: 0,
                });
            }
        }
    }

    fn set_endpoint(&mut self, target: Endpoint, airport: &'static Airport) {
        let other = match target {
            Endpoint::Origin => self.form.dest,
            Endpoint::Dest => self.form.origin,
        };
        if other.is_some_and(|o| o.iata == airport.iata) {
            self.set_status(
                Level::Warn,
                format!("{} IS ALREADY THE OTHER END OF THE ROUTE.", airport.iata),
            );
            return;
        }
        match target {
            Endpoint::Origin => self.form.origin = Some(airport),
            Endpoint::Dest => self.form.dest = Some(airport),
        }
        let role = if target == Endpoint::Origin {
            "ORIGIN"
        } else {
            "DESTINATION"
        };
        self.set_status(Level::Good, format!("{role} LOCKED: {}", airport.long()));
        self.spin = false;
        if self.form.origin.is_some() && self.form.dest.is_some() {
            self.refit();
        }
    }

    /// Zoom toward the pointer, or toward the middle of the pane when
    /// zooming from the keyboard.
    fn zoom_at(&mut self, factor: f64, pos: Option<Position>) {
        let (w, h) = self.globe_dots();
        self.release_view();
        let (x, y) = match pos.filter(|p| self.hit.globe_inner.contains(*p)) {
            Some(p) => self.cell_to_dots(p),
            None => (w / 2.0, h / 2.0),
        };
        self.view.zoom_about(factor, x, y, w, h);
    }

    fn toggle_projection(&mut self) {
        self.view.proj = match self.view.proj {
            Projection::Globe => Projection::Map,
            Projection::Map => Projection::Globe,
        };
        self.view.clamp();
        self.spin = false;
        if self.route_fit().is_some() {
            self.refit();
        }
    }

    // ------------------------------------------------------------------- tick

    /// Advance animation by `dt` seconds and collect finished searches.
    pub fn on_tick(&mut self, dt: f64) {
        self.tick += 1;
        self.clock += dt;
        self.drain();
        // A resized pane (or the very first frame) needs a fresh framing,
        // unless the user has positioned the globe themselves.
        let pane = (self.hit.globe_inner.width, self.hit.globe_inner.height);
        if self.framed && pane.0 >= 8 && pane != self.framed_for {
            self.refit();
            self.framed_for = pane;
        }
        if let Some(goal) = self.goal {
            // Exponential ease: the same glide whatever the frame rate.
            let ease = 1.0 - (-5.0 * dt).exp();
            let dlon = wrap_lon(goal.lon0 - self.view.lon0);
            self.view.lat0 += (goal.lat0 - self.view.lat0) * ease;
            self.view.lon0 = wrap_lon(self.view.lon0 + dlon * ease);
            self.view.zoom += (goal.zoom - self.view.zoom) * ease;
            // The glide turns and resizes the planet. Where it sits in the
            // pane is the user's choice and is left alone.
            let arrived = (goal.lat0 - self.view.lat0).abs() < 0.05
                && dlon.abs() < 0.05
                && (goal.zoom - self.view.zoom).abs() < 0.005;
            if arrived {
                (self.view.lat0, self.view.lon0, self.view.zoom) =
                    (goal.lat0, goal.lon0, goal.zoom);
                self.goal = None;
            }
            self.view.clamp();
        } else if self.spin {
            self.view.lon0 = wrap_lon(self.view.lon0 + 6.0 * dt);
        }
    }

    /// Block until every pending search has answered, then let animations
    /// finish. For snapshots and tests, where there is no event loop.
    pub fn settle(&mut self) {
        let deadline = Instant::now() + StdDuration::from_secs(30);
        while (self.inflight > 0 || self.survey_left > 0) && Instant::now() < deadline {
            self.drain();
            thread::sleep(StdDuration::from_millis(2));
        }
        for _ in 0..80 {
            self.on_tick(0.05);
        }
    }

    /// Write the settings, and the form as it stands, to the settings file.
    /// Every change does this by itself; `false` means it could not.
    pub fn persist(&mut self) -> bool {
        if self.ephemeral {
            return false;
        }
        self.cfg.mission = self.form.to_session();
        match self.cfg.save() {
            Ok(path) => {
                self.saved = Some((path, Local::now().time()));
                true
            }
            Err(e) => {
                self.set_status(Level::Alert, format!("COULD NOT SAVE SETTINGS: {e}"));
                false
            }
        }
    }

    // --------------------------------------------------------------- settings

    pub fn open_settings(&mut self) {
        self.popup = Some(Popup::Settings { sel: 0 });
    }

    /// Straight to the key box; the settings screen is where it leads back.
    fn open_key_entry(&mut self) {
        self.settings_return = Some(0);
        self.popup = Some(Popup::Text {
            kind: TextKind::ApiKey,
            input: String::new(),
            error: None,
        });
    }

    /// A picker opened from the settings screen hands back to it when done.
    fn resume_settings(&mut self) {
        if self.popup.is_none() {
            if let Some(sel) = self.settings_return.take() {
                self.popup = Some(Popup::Settings { sel });
            }
        }
    }

    /// Enter on a settings row: open its editor, or step its value.
    fn activate_setting(&mut self, row: Setting) {
        let at = Setting::ALL.iter().position(|s| *s == row).unwrap_or(0);
        match row {
            Setting::ApiKey => self.open_key_entry(),
            Setting::Currency => {
                self.settings_return = Some(at);
                self.open_currency_picker();
            }
            _ => self.adjust_setting(row, 1),
        }
    }

    /// Left or right on a settings row. Each change is saved as it is made.
    fn adjust_setting(&mut self, row: Setting, step: i32) {
        fn cycle<T: Copy + PartialEq>(all: &[T], current: T, step: i32) -> T {
            let at = all.iter().position(|v| *v == current).unwrap_or(0) as i32;
            all[(at + step).rem_euclid(all.len() as i32) as usize]
        }
        match row {
            // These two have an editor of their own, opened with Enter.
            Setting::ApiKey | Setting::Currency => return,
            Setting::Theme => {
                self.theme = self.theme.next();
                self.cfg.theme = self.theme.name.to_lowercase();
            }
            Setting::Charset => {
                let now = self.cfg.charset.to_ascii_lowercase();
                self.cfg.charset = cycle(&["auto", "rich", "safe"], now.as_str(), step).to_string();
                self.set_status(
                    Level::Info,
                    "SYMBOLS ARE CHOSEN AT START-UP: THIS TAKES EFFECT NEXT TIME.",
                );
            }
            Setting::Continents => self.cfg.continents = !self.cfg.continents,
            Setting::Borders => self.cfg.borders = !self.cfg.borders,
            Setting::TextSize => return self.resize_text(step),
            Setting::CacheHours => {
                self.cfg.cache_hours = cycle(&CACHE_CHOICES, self.cfg.cache_hours, step);
                self.set_status(
                    Level::Info,
                    "HOW LONG RESULTS ARE KEPT TAKES EFFECT NEXT TIME THE PROGRAM STARTS.",
                );
            }
        }
        self.persist();
    }

    /// `S` on the settings screen: write the file now, and say where.
    fn save_settings(&mut self) {
        if self.ephemeral {
            self.set_status(
                Level::Warn,
                "A SCRIPTED RUN LEAVES THE SETTINGS FILE ALONE. NOTHING WRITTEN.",
            );
        } else if self.persist() {
            let place = self
                .saved
                .as_ref()
                .map_or(String::new(), |(path, _)| path.display().to_string());
            self.set_status(Level::Good, format!("SETTINGS SAVED: {place}"));
        }
    }

    /// `T` on the settings screen: ask the service about the key again.
    fn check_key(&mut self) {
        if self.live {
            self.key_state = KeyState::Checking;
            self.worker.submit(Job::Quota);
            self.set_status(Level::Info, "CHECKING THE KEY. THIS COSTS NO SEARCHES.");
        } else {
            self.set_status(
                Level::Warn,
                "NO KEY TO CHECK. PRESS ENTER ON API KEY TO ADD ONE.",
            );
        }
    }

    // ------------------------------------------------------------------- keys

    pub fn on_key(&mut self, key: KeyEvent) {
        self.last_input = self.clock;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(key.code, KeyCode::Char('c' | 'q')) {
            self.quit = true;
            return;
        }
        // Paste. Most terminals do this themselves and hand over the text
        // (`on_paste`). The classic Windows console does not: with a
        // full-screen program running, Ctrl+V arrives here as a keystroke,
        // so the clipboard is read directly.
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let paste = (ctrl && !alt && matches!(key.code, KeyCode::Char('v' | 'V')))
            || (key.modifiers.contains(KeyModifiers::SHIFT) && key.code == KeyCode::Insert);
        if paste {
            if self.popup.is_some() {
                if let Some(text) = (self.clipboard)() {
                    self.on_paste(text);
                }
            }
            return;
        }
        // No other Ctrl chord means anything here, and none must be taken
        // for its bare letter: Ctrl+S is not "scan". (Ctrl with Alt is how
        // AltGr arrives on Windows, and types an ordinary character.)
        if ctrl && !alt {
            return;
        }
        if self.popup.is_some() {
            self.popup_key(key);
            self.resume_settings();
            return;
        }
        if self.intel.is_some() && self.intel_key(key.code) {
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') | KeyCode::F(1) => self.popup = Some(Popup::Help),
            KeyCode::Tab => self.cycle_focus(1),
            KeyCode::BackTab => self.cycle_focus(-1),
            KeyCode::Char('s') | KeyCode::F(5) => self.launch(),
            KeyCode::Char('i') => self.toggle_intel(),
            KeyCode::Char('v') => self.open_survey(),
            KeyCode::Char('g') => self.toggle_projection(),
            KeyCode::Char('f') => self.globe_full = !self.globe_full,
            KeyCode::Char('0') => {
                // Back to a plain centred globe.
                self.release_view();
                (self.view.zoom, self.view.drop, self.view.shift) = (1.0, 0.0, 0.0);
                self.view.clamp();
            }
            KeyCode::Char('m') => {
                self.marker = self.marker.next();
                self.set_status(
                    Level::Info,
                    format!("GLOBE MARKER: {}", self.marker.label()),
                );
            }
            KeyCode::Char('b') => {
                self.cfg.borders = !self.cfg.borders;
                self.persist();
            }
            KeyCode::Char('p') => {
                self.cfg.continents = !self.cfg.continents;
                let state = if self.cfg.continents { "ON" } else { "OFF" };
                self.set_status(Level::Info, format!("CONTINENT COLOURS {state}"));
                self.persist();
            }
            KeyCode::Char('t') => {
                self.theme = self.theme.next();
                self.cfg.theme = self.theme.name.to_lowercase();
                self.persist();
            }
            KeyCode::F(2) => self.open_settings(),
            KeyCode::Char('K') => self.open_key_entry(),
            KeyCode::Char('o') => self.open_booking(),
            KeyCode::Char('x') => self.swap_endpoints(),
            KeyCode::Char('$') => self.open_currency_picker(),
            KeyCode::Char('>' | '.') => self.resize_text(-1),
            KeyCode::Char('<' | ',') => self.resize_text(1),
            KeyCode::Char('c') => self.refit(),
            KeyCode::Char('+' | '=') => self.zoom_at(1.25, None),
            KeyCode::Char('-' | '_') => self.zoom_at(0.8, None),
            KeyCode::Char('[') => self.step_date(-1),
            KeyCode::Char(']') => self.step_date(1),
            KeyCode::Char(c @ '1'..='7') => self.set_sort(Sort::ALL[c as usize - '1' as usize]),
            KeyCode::Char(' ') => {
                if self.selected().is_some() {
                    self.popup = Some(Popup::Dossier { scroll: 0 });
                }
            }
            KeyCode::Esc => {
                if self.globe_full {
                    self.globe_full = false;
                } else {
                    self.leave_return_stage();
                }
            }
            code => {
                // Vim-style movement mirrors the arrow keys.
                let code = match code {
                    KeyCode::Char('h') => KeyCode::Left,
                    KeyCode::Char('j') => KeyCode::Down,
                    KeyCode::Char('k') => KeyCode::Up,
                    KeyCode::Char('l') => KeyCode::Right,
                    other => other,
                };
                match self.focus {
                    Pane::Form => self.form_key(code),
                    Pane::Radar => self.radar_key(code),
                    Pane::Tracks => self.tracks_key(code),
                    Pane::Globe => {
                        self.globe_key(code, key.modifiers.contains(KeyModifiers::SHIFT))
                    }
                }
            }
        }
    }

    fn cycle_focus(&mut self, step: i64) {
        const ORDER: [Pane; 4] = [Pane::Form, Pane::Tracks, Pane::Radar, Pane::Globe];
        let at = ORDER.iter().position(|p| *p == self.focus).unwrap_or(0) as i64;
        self.focus = ORDER[(at + step).rem_euclid(ORDER.len() as i64) as usize];
    }

    fn swap_endpoints(&mut self) {
        std::mem::swap(&mut self.form.origin, &mut self.form.dest);
        self.set_status(Level::Info, "ORIGIN AND DESTINATION SWAPPED.");
        self.refit();
    }

    /// Make the text smaller (`-1`) or larger. The map is drawn in dots,
    /// eight to a character, so smaller text is the one way to a finer map.
    fn resize_text(&mut self, step: i32) {
        match console::resize_text(step) {
            Some(pixels) => {
                self.cfg.text_size = pixels;
                self.persist();
                self.set_status(
                    Level::Info,
                    format!("TEXT {pixels} PIXELS HIGH. SMALLER TEXT, FINER MAP:  >  FINER   <  COARSER"),
                );
            }
            // Only the classic Windows console lets a program do this.
            None => self.set_status(
                Level::Info,
                "THIS TERMINAL SETS ITS OWN TEXT SIZE. FOR A FINER MAP PRESS CTRL AND MINUS (CMD AND MINUS ON A MAC).",
            ),
        }
    }

    pub fn open_currency_picker(&mut self) {
        // Open on the currency in use, so Enter straight away changes nothing.
        let sel = currency::ALL
            .iter()
            .position(|c| c.code == self.cfg.currency)
            .unwrap_or(0);
        self.popup = Some(Popup::Currency {
            input: String::new(),
            sel,
        });
    }

    /// Switch the currency fares are quoted in.
    fn set_currency(&mut self, code: String) {
        if code == self.cfg.currency {
            self.set_status(Level::Info, format!("CURRENCY STAYS {code}."));
            return;
        }
        self.cfg.currency = code.clone();
        // Fares on the board were quoted in the old currency. Showing them
        // under a new symbol would be a lie, so they go; the fare log keeps
        // each currency's history apart and loses nothing.
        let had_results = !self.board.slots.is_empty();
        self.generation = self.worker.next_generation();
        self.board.slots.clear();
        self.board.ret = None;
        self.searched = None;
        self.inflight = 0;
        self.survey_left = 0;
        let name = currency::get(&code)
            .map_or("AS TYPED", |c| c.name)
            .to_uppercase();
        self.set_status(Level::Good, format!("CURRENCY: {code} ({name})."));
        self.persist();
        if had_results && self.intel.is_none() {
            if self.live {
                // A re-scan spends searches, so it stays the user's call.
                self.set_status(
                    Level::Good,
                    format!("CURRENCY: {code} ({name}). PRESS S TO RE-SCAN IN IT."),
                );
            } else {
                self.launch();
            }
        }
    }

    fn form_key(&mut self, code: KeyCode) {
        let last = self.form.fields().len() - 1;
        match code {
            KeyCode::Up => self.form.cursor = self.form.cursor.min(last).saturating_sub(1),
            KeyCode::Down => self.form.cursor = (self.form.cursor + 1).min(last),
            KeyCode::Home => self.form.cursor = 0,
            KeyCode::End => self.form.cursor = last,
            KeyCode::Left => self.adjust_field(-1),
            KeyCode::Right => self.adjust_field(1),
            KeyCode::Enter => self.edit_field(),
            KeyCode::Backspace | KeyCode::Delete => self.clear_field(),
            _ => {}
        }
    }

    /// Left/right on a form row: step its value.
    fn adjust_field(&mut self, step: i64) {
        fn cycle<T: Copy + PartialEq>(all: &[T], current: T, step: i64) -> T {
            let at = all.iter().position(|v| *v == current).unwrap_or(0) as i64;
            all[(at + step).rem_euclid(all.len() as i64) as usize]
        }
        let presets: Vec<TimeWindow> = TimeWindow::PRESETS.iter().map(|(w, _)| *w).collect();
        match self.form.field() {
            Field::Trip => {
                self.form.round_trip = !self.form.round_trip;
                self.form.cursor = self.form.cursor.min(self.form.fields().len() - 1);
            }
            Field::Depart => self.set_depart(self.form.depart + Duration::days(step)),
            Field::Return => self.set_return(self.form.ret + Duration::days(step)),
            Field::Flex => {
                (self.form.flex, self.form.flex_later) =
                    cycle(&FLEX_CHOICES, (self.form.flex, self.form.flex_later), step);
            }
            Field::DepWindow => self.form.dep_window = cycle(&presets, self.form.dep_window, step),
            Field::ArrWindow => self.form.arr_window = cycle(&presets, self.form.arr_window, step),
            Field::Stops => self.form.stops = cycle(&Stops::ALL, self.form.stops, step),
            Field::Cabin => self.form.cabin = cycle(&Cabin::ALL, self.form.cabin, step),
            Field::Pax => self.form.adults = (i64::from(self.form.adults) + step).clamp(1, 9) as u8,
            Field::From | Field::To | Field::Airlines | Field::Launch => {}
        }
    }

    /// Enter on a form row: open its editor, or step it if it has none.
    fn edit_field(&mut self) {
        let text = |kind, input: String| {
            Some(Popup::Text {
                kind,
                input,
                error: None,
            })
        };
        let window_text = |w: TimeWindow| {
            if w == TimeWindow::ANY {
                String::new()
            } else {
                w.label()
            }
        };
        match self.form.field() {
            Field::From => self.open_airport_picker(Endpoint::Origin),
            Field::To => self.open_airport_picker(Endpoint::Dest),
            Field::Depart => {
                self.popup = Some(Popup::Calendar {
                    ret: false,
                    cursor: self.form.depart,
                })
            }
            Field::Return => {
                self.popup = Some(Popup::Calendar {
                    ret: true,
                    cursor: self.form.ret,
                })
            }
            Field::DepWindow => {
                self.popup = text(TextKind::DepWindow, window_text(self.form.dep_window))
            }
            Field::ArrWindow => {
                self.popup = text(TextKind::ArrWindow, window_text(self.form.arr_window))
            }
            Field::Airlines => self.popup = text(TextKind::Airlines, self.form.airlines.join(",")),
            Field::Launch => self.launch(),
            _ => self.adjust_field(1),
        }
    }

    fn clear_field(&mut self) {
        match self.form.field() {
            Field::From => self.form.origin = None,
            Field::To => self.form.dest = None,
            Field::DepWindow => self.form.dep_window = TimeWindow::ANY,
            Field::ArrWindow => self.form.arr_window = TimeWindow::ANY,
            Field::Airlines => self.form.airlines.clear(),
            Field::Flex => (self.form.flex, self.form.flex_later) = (0, false),
            // The soonest a trip can sensibly start: tomorrow.
            Field::Depart => self.set_depart(Local::now().date_naive() + Duration::days(1)),
            _ => {}
        }
    }

    fn open_airport_picker(&mut self, target: Endpoint) {
        self.popup = Some(Popup::Airport {
            target,
            input: String::new(),
            hits: Vec::new(),
            sel: 0,
        });
    }

    /// Move the departure date, keeping the trip the same length.
    fn set_depart(&mut self, date: NaiveDate) {
        let length = self.form.ret - self.form.depart;
        self.form.depart = date.max(Local::now().date_naive());
        self.form.ret = self.form.depart + length;
    }

    fn set_return(&mut self, date: NaiveDate) {
        self.form.ret = date.max(self.form.depart);
    }

    fn radar_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Up => self.step_date(-1),
            KeyCode::Down => self.step_date(1),
            KeyCode::Enter => self.focus = Pane::Tracks,
            _ => {}
        }
    }

    fn tracks_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Up => self.move_sel(-1),
            KeyCode::Down => self.move_sel(1),
            KeyCode::PageUp => self.move_sel(-8),
            KeyCode::PageDown => self.move_sel(8),
            KeyCode::Home => self.move_sel(-10_000),
            KeyCode::End => self.move_sel(10_000),
            KeyCode::Left => self.step_date(-1),
            KeyCode::Right => self.step_date(1),
            KeyCode::Enter => self.activate_track(),
            KeyCode::Backspace => self.leave_return_stage(),
            _ => {}
        }
    }

    /// Arrow keys turn the globe; with Shift they move the planet itself.
    fn globe_key(&mut self, code: KeyCode, shift: bool) {
        let (w, h) = self.globe_dots();
        let step = w.min(h).max(8.0) / 10.0;
        let (dx, dy) = match code {
            KeyCode::Left => (step, 0.0),
            KeyCode::Right => (-step, 0.0),
            KeyCode::Up => (0.0, step),
            KeyCode::Down => (0.0, -step),
            _ => return,
        };
        let (w, h) = (w.max(8.0), h.max(8.0));
        if shift && self.view.proj == Projection::Globe {
            // Turning drags the surface, so Left brings the east into view.
            // Moving carries the planet the way the arrow points.
            self.view.slide(-dx, -dy, w, h);
        } else {
            self.release_view();
            self.view.pan(dx, dy, w, h);
        }
    }

    // ------------------------------------------------------------------ intel

    fn toggle_intel(&mut self) {
        if self.intel.take().is_some() {
            return;
        }
        self.intel = Some(Intel {
            cursor: self.form.depart,
        });
        // The point of this screen is the year at a glance. If this route
        // has barely been sampled, fill the year in: straight away when it
        // is free, and by asking (once per route) when it costs searches.
        let today = Local::now().date_naive();
        let Some(route) = self.route_key() else {
            return;
        };
        // "Sampled" means spread across the year, not merely numerous: a
        // two-week scan logs fifteen fares and says nothing about July.
        let months: std::collections::HashSet<(i32, u32)> = self
            .fares
            .latest(&route)
            .keys()
            .filter(|day| **day >= today)
            .map(|day| (day.year(), day.month()))
            .collect();
        if months.len() >= 8 || self.survey_left > 0 {
            return;
        }
        if !self.live {
            self.launch_survey(YEAR);
        } else if self.survey_offered.as_ref() != Some(&route) {
            self.survey_offered = Some(route);
            self.popup = Some(Popup::Survey { horizon: YEAR });
        }
    }

    fn open_survey(&mut self) {
        if self.form_query(self.form.depart).is_none() {
            self.set_status(Level::Alert, "SET BOTH FROM AND TO BEFORE SURVEYING.");
            return;
        }
        if self.intel.is_none() {
            self.intel = Some(Intel {
                cursor: self.form.depart,
            });
        }
        self.popup = Some(Popup::Survey { horizon: YEAR });
    }

    /// Keys for the INTEL screen. Returns false to fall through to globals.
    fn intel_key(&mut self, code: KeyCode) -> bool {
        let today = Local::now().date_naive();
        let last = today + Duration::days(HORIZONS[YEAR].1);
        let Some(intel) = &mut self.intel else {
            return false;
        };
        let moved = match code {
            KeyCode::Left | KeyCode::Char('h') => Some(intel.cursor - Duration::days(7)),
            KeyCode::Right | KeyCode::Char('l') => Some(intel.cursor + Duration::days(7)),
            KeyCode::Up | KeyCode::Char('k') => Some(intel.cursor - Duration::days(1)),
            KeyCode::Down | KeyCode::Char('j') => Some(intel.cursor + Duration::days(1)),
            KeyCode::PageUp => intel.cursor.checked_sub_months(Months::new(1)),
            KeyCode::PageDown => intel.cursor.checked_add_months(Months::new(1)),
            KeyCode::Home => Some(today),
            _ => None,
        };
        if let Some(date) = moved {
            intel.cursor = date.clamp(today, last);
            return true;
        }
        match code {
            // On this screen the question is not "which key?" but "what am
            // I looking at?".
            KeyCode::Char('?') | KeyCode::F(1) => {
                self.popup = Some(Popup::Guide);
                true
            }
            KeyCode::Enter => {
                let date = intel.cursor;
                self.intel = None;
                self.set_depart(date);
                self.focus = Pane::Form;
                self.set_status(
                    Level::Good,
                    format!("DEPARTURE SET TO {}. PRESS S TO SCAN.", fmt::day(date)),
                );
                true
            }
            KeyCode::Esc => {
                self.intel = None;
                true
            }
            // Pane-local keys make no sense here; swallow them.
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Char(' ' | '[' | ']' | '1'..='7') => true,
            _ => false,
        }
    }

    // ----------------------------------------------------------------- popups

    fn popup_key(&mut self, key: KeyEvent) {
        let today = Local::now().date_naive();
        let Some(popup) = &mut self.popup else { return };
        match popup {
            Popup::Help | Popup::Guide => self.popup = None,
            Popup::Dossier { scroll } => match key.code {
                KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => *scroll += 1,
                KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
                KeyCode::PageDown => *scroll += 10,
                KeyCode::Home => *scroll = 0,
                KeyCode::Char('o') => self.open_booking(),
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char(' ' | 'q') => self.popup = None,
                _ => {}
            },
            Popup::Currency { input, sel } => {
                let hits = currency::search(input);
                let last = hits.len().saturating_sub(1);
                match key.code {
                    KeyCode::Esc => self.popup = None,
                    KeyCode::Up => *sel = sel.saturating_sub(1),
                    KeyCode::Down => *sel = (*sel + 1).min(last),
                    KeyCode::PageUp => *sel = sel.saturating_sub(8),
                    KeyCode::PageDown => *sel = (*sel + 8).min(last),
                    KeyCode::Home => *sel = 0,
                    KeyCode::End => *sel = last,
                    KeyCode::Enter => {
                        // A listed currency if one is highlighted; failing
                        // that, whatever 3-letter code was typed.
                        let choice = hits
                            .get(*sel)
                            .map(|c| c.code.to_string())
                            .or_else(|| currency::normalize(input));
                        if let Some(code) = choice {
                            self.popup = None;
                            self.set_currency(code);
                        }
                    }
                    KeyCode::Backspace => {
                        input.pop();
                        *sel = 0;
                    }
                    KeyCode::Char(c)
                        if input.len() < 16 && (c.is_ascii_alphabetic() || c == ' ') =>
                    {
                        input.push(c);
                        *sel = 0;
                    }
                    _ => {}
                }
            }
            Popup::Settings { sel } => {
                let last = Setting::ALL.len() - 1;
                let row = Setting::ALL[(*sel).min(last)];
                match key.code {
                    KeyCode::Esc | KeyCode::F(2) | KeyCode::Char('q') => self.popup = None,
                    KeyCode::Up | KeyCode::Char('k') => *sel = sel.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                        *sel = (*sel + 1).min(last)
                    }
                    KeyCode::Home => *sel = 0,
                    KeyCode::End => *sel = last,
                    KeyCode::Left | KeyCode::Char('h') => self.adjust_setting(row, -1),
                    KeyCode::Right | KeyCode::Char('l') => self.adjust_setting(row, 1),
                    KeyCode::Enter | KeyCode::Char(' ') => self.activate_setting(row),
                    KeyCode::Char('s') => self.save_settings(),
                    KeyCode::Char('t') => self.check_key(),
                    _ => {}
                }
            }
            Popup::Survey { horizon } => match key.code {
                KeyCode::Left | KeyCode::Char('h') => *horizon = horizon.saturating_sub(1),
                KeyCode::Right | KeyCode::Char('l') => *horizon = (*horizon + 1).min(YEAR),
                KeyCode::Enter => {
                    let horizon = *horizon;
                    self.popup = None;
                    self.launch_survey(horizon);
                }
                KeyCode::Esc | KeyCode::Char('q') => self.popup = None,
                _ => {}
            },
            Popup::Airport {
                target,
                input,
                hits,
                sel,
            } => match key.code {
                KeyCode::Esc => self.popup = None,
                KeyCode::Up => *sel = sel.saturating_sub(1),
                KeyCode::Down => *sel = (*sel + 1).min(hits.len().saturating_sub(1)),
                KeyCode::Enter => {
                    if let Some(airport) = hits.get(*sel).copied() {
                        let target = *target;
                        self.popup = None;
                        self.set_endpoint(target, airport);
                    }
                }
                KeyCode::Backspace => {
                    input.pop();
                    *hits = airports::db().search(input, 12);
                    *sel = 0;
                }
                KeyCode::Char(c) if input.len() < 40 => {
                    input.push(c);
                    *hits = airports::db().search(input, 12);
                    *sel = 0;
                }
                _ => {}
            },
            Popup::Calendar { ret, cursor } => {
                let floor = if *ret { self.form.depart } else { today };
                let moved = match key.code {
                    KeyCode::Left | KeyCode::Char('h') => Some(*cursor - Duration::days(1)),
                    KeyCode::Right | KeyCode::Char('l') => Some(*cursor + Duration::days(1)),
                    KeyCode::Up | KeyCode::Char('k') => Some(*cursor - Duration::days(7)),
                    KeyCode::Down | KeyCode::Char('j') => Some(*cursor + Duration::days(7)),
                    KeyCode::PageUp => cursor.checked_sub_months(Months::new(1)),
                    KeyCode::PageDown => cursor.checked_add_months(Months::new(1)),
                    KeyCode::Home => Some(floor),
                    _ => None,
                };
                if let Some(date) = moved {
                    *cursor = date.max(floor);
                    return;
                }
                match key.code {
                    KeyCode::Enter => {
                        let (ret, date) = (*ret, *cursor);
                        self.popup = None;
                        if ret {
                            self.set_return(date)
                        } else {
                            self.set_depart(date)
                        }
                    }
                    KeyCode::Esc | KeyCode::Char('q') => self.popup = None,
                    _ => {}
                }
            }
            Popup::Text { input, error, .. } => match key.code {
                KeyCode::Esc => self.popup = None,
                KeyCode::Enter => self.submit_text(),
                KeyCode::Backspace => {
                    input.pop();
                    *error = None;
                }
                KeyCode::Char(c) if input.len() < 160 => {
                    input.push(c);
                    *error = None;
                }
                _ => {}
            },
        }
    }

    pub fn on_paste(&mut self, text: String) {
        self.last_input = self.clock;
        let clean: String = text.chars().filter(|c| !c.is_control()).collect();
        match &mut self.popup {
            Some(Popup::Text { input, error, .. }) => {
                input.push_str(&clean);
                input.truncate(160);
                *error = None;
            }
            Some(Popup::Airport {
                input, hits, sel, ..
            }) => {
                input.push_str(&clean);
                input.truncate(40);
                *hits = airports::db().search(input, 12);
                *sel = 0;
            }
            Some(Popup::Currency { input, sel }) => {
                input.extend(
                    clean
                        .chars()
                        .filter(|c| c.is_ascii_alphabetic() || *c == ' '),
                );
                input.truncate(16);
                *sel = 0;
            }
            _ => {}
        }
    }

    fn submit_text(&mut self) {
        let Some(Popup::Text { kind, input, .. }) = &self.popup else {
            return;
        };
        let (kind, input) = (*kind, input.trim().to_string());
        let outcome: Result<(), String> = match kind {
            TextKind::DepWindow => TimeWindow::parse(&input).map(|w| self.form.dep_window = w),
            TextKind::ArrWindow => TimeWindow::parse(&input).map(|w| self.form.arr_window = w),
            TextKind::Airlines => parse_airlines(&input).map(|list| self.form.airlines = list),
            TextKind::ApiKey => check_key_text(&input).map(|()| self.install_key(&input)),
        };
        match outcome {
            Ok(()) => self.popup = None,
            Err(message) => {
                if let Some(Popup::Text { error, .. }) = &mut self.popup {
                    *error = Some(message);
                }
            }
        }
    }

    /// Switch data source: a key arms live fares, an empty one returns to
    /// the simulator. Old results are cleared so the two never share a screen.
    fn install_key(&mut self, key: &str) {
        self.cfg.api_key = key.to_string();
        self.persist();
        self.generation = self.worker.next_generation();
        self.board.slots.clear();
        self.board.ret = None;
        self.searched = None;
        self.inflight = 0;
        self.survey_left = 0;
        self.quota = None;
        self.key_tail = tail(key);
        if key.is_empty() {
            self.worker
                .submit(Job::Swap(Box::new(Demo::new(self.latency))));
            (self.live, self.source, self.key_source) = (false, "SIMULATOR", None);
            self.key_state = KeyState::Absent;
            self.set_status(Level::Warn, "KEY REMOVED. BACK TO SIMULATED FARES.");
        } else {
            self.worker.submit(Job::Swap(Box::new(SerpApi::new(
                key.to_string(),
                self.dump.clone(),
            ))));
            // Asking after the account is free, and settles at once
            // whether the key is any good: `on_quota` reports the answer.
            self.worker.submit(Job::Quota);
            (self.live, self.source, self.key_source) = (true, "SERPAPI", Some(KeySource::File));
            self.key_state = KeyState::Checking;
            self.set_status(Level::Info, "KEY STORED. CHECKING IT WITH SERPAPI...");
        }
    }

    // ------------------------------------------------------------------ mouse

    pub fn on_mouse(&mut self, event: MouseEvent) {
        let pos = Position::new(event.column, event.row);
        match event.kind {
            MouseEventKind::ScrollUp => return self.on_scroll(pos, -1),
            MouseEventKind::ScrollDown => return self.on_scroll(pos, 1),
            MouseEventKind::Moved => {
                // A bare move means no button is held: if we thought a drag
                // was in progress, its release happened outside the window.
                self.drag = None;
                let found = if self.popup.is_none() && self.intel.is_none() {
                    self.airports_at(pos)
                } else {
                    Vec::new()
                };
                self.hover = found.first().copied();
                self.hover_more = found.len().saturating_sub(1);
                return;
            }
            _ => self.last_input = self.clock,
        }
        if self.popup.is_some() {
            match event.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    let double = self.is_double_click(pos);
                    self.popup_click(pos, double);
                    self.resume_settings();
                }
                // The Windows console habit: right-click pastes.
                MouseEventKind::Down(MouseButton::Right) => {
                    if let Some(text) = (self.clipboard)() {
                        self.on_paste(text);
                    }
                }
                _ => {}
            }
            return;
        }
        // The header bar is on every screen, so its buttons are clickable
        // from every screen.
        if event.kind == MouseEventKind::Down(MouseButton::Left) {
            if self.hit.currency.contains(pos) {
                return self.open_currency_picker();
            }
            if self.hit.setup.contains(pos) {
                return self.open_settings();
            }
        }
        if self.intel.is_some() {
            if let MouseEventKind::Down(MouseButton::Left) = event.kind {
                self.intel_click(pos);
            }
            return;
        }
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let double = self.is_double_click(pos);
                self.left_down(pos, double);
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                // The drag keeps hold of the globe even when the pointer
                // strays outside its pane.
                if let Some(drag) = &mut self.drag {
                    let dx = f64::from(event.column) - f64::from(drag.last.0);
                    let dy = f64::from(event.row) - f64::from(drag.last.1);
                    if dx != 0.0 || dy != 0.0 {
                        drag.moved = true;
                        drag.last = (event.column, event.row);
                        let (w, h) = self.globe_dots();
                        // A cell is two dots wide and four tall.
                        let (dx, dy) = (dx * 2.0, dy * 4.0);
                        // Held with Shift, the drag moves the planet across
                        // the pane; without, it turns it. Ctrl does the same
                        // as Shift, because many terminals keep Shift+drag
                        // for selecting text and never pass it on.
                        let moving = event
                            .modifiers
                            .intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL);
                        if moving && self.view.proj == Projection::Globe {
                            self.view.slide(dx, dy, w, h);
                        } else {
                            self.framed = false;
                            self.view.pan(dx, dy, w, h);
                        }
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                // Press and release without movement is a click: pick an airport.
                if self.drag.take().is_some_and(|d| !d.moved) {
                    self.pick_at(pos, Endpoint::Dest);
                }
            }
            MouseEventKind::Down(MouseButton::Right) if self.hit.globe_inner.contains(pos) => {
                self.pick_at(pos, Endpoint::Origin);
            }
            _ => {}
        }
    }

    /// Wheel input: `steps` is the net movement since the last frame,
    /// negative for up. Terminals disagree wildly on how many events one
    /// notch produces (1 to 30+), so the effect is clamped per frame.
    pub fn on_scroll(&mut self, pos: Position, steps: i32) {
        if steps == 0 {
            return;
        }
        self.last_input = self.clock;
        let rows = i64::from(steps.clamp(-4, 4));
        match &mut self.popup {
            Some(Popup::Dossier { scroll }) => {
                *scroll = (i64::from(*scroll) + rows * 2).max(0) as u16;
                return;
            }
            Some(Popup::Airport { hits, sel, .. }) => {
                let last = hits.len().saturating_sub(1) as i64;
                *sel = (*sel as i64 + rows).clamp(0, last) as usize;
                return;
            }
            Some(Popup::Currency { input, sel }) => {
                let last = currency::search(input).len().saturating_sub(1) as i64;
                *sel = (*sel as i64 + rows).clamp(0, last) as usize;
                return;
            }
            Some(Popup::Calendar { .. }) => {
                let code = if rows < 0 {
                    KeyCode::PageUp
                } else {
                    KeyCode::PageDown
                };
                return self.popup_key(KeyEvent::new(code, KeyModifiers::NONE));
            }
            Some(_) => return,
            None => {}
        }
        if self.intel.is_some() {
            // One week per notch across the heatmap.
            let code = if rows < 0 {
                KeyCode::Left
            } else {
                KeyCode::Right
            };
            self.intel_key(code);
        } else if self.hit.globe.contains(pos) {
            self.zoom_at(1.18f64.powi(-steps.clamp(-3, 3)), Some(pos));
        } else if self.hit.tracks.contains(pos) {
            self.move_sel(rows);
        } else if self.hit.radar.contains(pos) {
            self.step_date(rows.signum());
        } else if self.hit.form.contains(pos) {
            let last = self.form.fields().len() as i64 - 1;
            self.form.cursor = (self.form.cursor as i64 + rows.signum()).clamp(0, last) as usize;
        }
    }

    /// Second press on the same cell within 450 ms. Activation needs a real
    /// double-click because some actions spend API quota; a stray single
    /// click must never do that.
    fn is_double_click(&mut self, pos: Position) -> bool {
        let now = Instant::now();
        let double = self.last_click.is_some_and(|(at, p)| {
            p == pos && now.duration_since(at) < StdDuration::from_millis(450)
        });
        self.last_click = if double { None } else { Some((now, pos)) };
        double
    }

    /// Single click focuses and selects; double click activates.
    fn left_down(&mut self, pos: Position, double: bool) {
        if self.hit.globe_inner.contains(pos) {
            self.focus = Pane::Globe;
            // Pressing only stops the glide; the view is not "taken in hand"
            // until the pointer actually drags.
            self.goal = None;
            self.spin = false;
            self.drag = Some(Drag {
                last: (pos.x, pos.y),
                moved: false,
            });
        } else if self.hit.form.contains(pos) {
            self.focus = Pane::Form;
            if self.hit.form_rows.contains(pos) {
                let row = usize::from(pos.y - self.hit.form_rows.y);
                if row < self.form.fields().len() {
                    self.form.cursor = row;
                    if double {
                        self.edit_field();
                    }
                }
            }
        } else if self.hit.radar.contains(pos) {
            self.focus = Pane::Radar;
            if self.hit.radar_rows.contains(pos) {
                let index = self.hit.radar_offset + usize::from(pos.y - self.hit.radar_rows.y);
                if let Some(date) = self.board.slots.keys().nth(index).copied() {
                    self.show_date(date);
                }
            }
        } else if self.hit.tracks.contains(pos) {
            self.focus = Pane::Tracks;
            if self.hit.track_head.contains(pos) {
                // A click on a column heading sorts by that column.
                let column = self
                    .hit
                    .track_cols
                    .iter()
                    .position(|(from, to)| (*from..*to).contains(&pos.x));
                if let Some(column) = column {
                    self.set_sort(TRACK_COLUMNS[column]);
                }
            } else if self.hit.track_rows.contains(pos) {
                let index = self.board.offset + usize::from(pos.y - self.hit.track_rows.y);
                if index < self.tracks().len() {
                    self.set_sel(index);
                    if double {
                        self.activate_track();
                    }
                }
            }
        }
    }

    fn intel_click(&mut self, pos: Position) {
        let (grid, Some(start)) = (self.hit.heat_grid, self.hit.heat_start) else {
            return;
        };
        if !grid.contains(pos) {
            return;
        }
        let week = i64::from((pos.x - grid.x) / self.hit.heat_cell.max(1));
        let weekday = i64::from(pos.y - grid.y);
        let date = start + Duration::days(week * 7 + weekday);
        let today = Local::now().date_naive();
        if let Some(intel) = &mut self.intel {
            if date >= today {
                intel.cursor = date;
            }
        }
    }

    fn popup_click(&mut self, pos: Position, double: bool) {
        let today = Local::now().date_naive();
        let (popup_area, list, grid) = (self.hit.popup, self.hit.popup_list, self.hit.cal_grid);
        let list_offset = self.hit.popup_list_offset;
        if !popup_area.contains(pos) {
            self.popup = None; // clicking outside dismisses
            return;
        }
        let Some(popup) = &mut self.popup else { return };
        match popup {
            Popup::Airport {
                target, hits, sel, ..
            } if list.contains(pos) => {
                let index = list_offset + usize::from(pos.y - list.y);
                if let Some(airport) = hits.get(index).copied() {
                    *sel = index;
                    if double {
                        let target = *target;
                        self.popup = None;
                        self.set_endpoint(target, airport);
                    }
                }
            }
            Popup::Currency { input, sel } if list.contains(pos) => {
                let index = list_offset + usize::from(pos.y - list.y);
                if let Some(choice) = currency::search(input).get(index) {
                    *sel = index;
                    if double {
                        let code = choice.code.to_string();
                        self.popup = None;
                        self.set_currency(code);
                    }
                }
            }
            Popup::Settings { sel } if list.contains(pos) => {
                let index = usize::from(pos.y - list.y);
                if let Some(row) = Setting::ALL.get(index).copied() {
                    // A click picks a row; a click on the row already
                    // picked (or a double-click) changes it.
                    let again = *sel == index || double;
                    *sel = index;
                    if again {
                        self.activate_setting(row);
                    }
                }
            }
            Popup::Calendar { ret, cursor } if grid.contains(pos) => {
                let floor = if *ret { self.form.depart } else { today };
                let first = cursor.with_day(1).unwrap_or(*cursor);
                let lead = i64::from(first.weekday().num_days_from_monday());
                let cell = i64::from(pos.y - grid.y) * 7 + i64::from((pos.x - grid.x) / 4);
                let date = first + Duration::days(cell - lead);
                if date.month() == first.month() && date >= floor {
                    *cursor = date;
                    if double {
                        let ret = *ret;
                        self.popup = None;
                        if ret {
                            self.set_return(date)
                        } else {
                            self.set_depart(date)
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------- scripted driving

    /// Apply one scripted action (see `--keys` in the README's testing notes).
    pub fn feed(&mut self, token: &str) {
        let press = |app: &mut App, code| app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
        let db = airports::db();
        if let Some(code) = token.strip_prefix("from:") {
            self.form.origin = db.get(code);
        } else if let Some(code) = token.strip_prefix("to:") {
            self.form.dest = db.get(code);
        } else if let Some(text) = token.strip_prefix("text:") {
            text.chars().for_each(|c| press(self, KeyCode::Char(c)));
        } else if let Some(text) = token.strip_prefix("paste:") {
            self.on_paste(text.to_string());
        } else {
            match token {
                "wait" => self.settle(),
                // Time for whatever is driving the app to get into position.
                "pause" => thread::sleep(StdDuration::from_millis(1500)),
                "enter" => press(self, KeyCode::Enter),
                "esc" => press(self, KeyCode::Esc),
                "tab" => press(self, KeyCode::Tab),
                "up" => press(self, KeyCode::Up),
                "down" => press(self, KeyCode::Down),
                "left" => press(self, KeyCode::Left),
                "right" => press(self, KeyCode::Right),
                "shift+up" | "shift+down" | "shift+left" | "shift+right" => {
                    let code = match token {
                        "shift+up" => KeyCode::Up,
                        "shift+down" => KeyCode::Down,
                        "shift+left" => KeyCode::Left,
                        _ => KeyCode::Right,
                    };
                    self.on_key(KeyEvent::new(code, KeyModifiers::SHIFT));
                }
                "space" => press(self, KeyCode::Char(' ')),
                "bksp" => press(self, KeyCode::Backspace),
                "pgup" => press(self, KeyCode::PageUp),
                "pgdn" => press(self, KeyCode::PageDown),
                "f2" => press(self, KeyCode::F(2)),
                other => other.chars().for_each(|c| press(self, KeyCode::Char(c))),
            }
        }
    }
}

/// The last four characters of a key, for showing which one is in use.
fn tail(key: &str) -> String {
    let count = key.chars().count();
    key.chars().skip(count.saturating_sub(4)).collect()
}

/// An empty key means "remove the key". Anything else must at least look
/// like one: a slip of the paste would otherwise go unnoticed until every
/// search failed.
fn check_key_text(key: &str) -> Result<(), String> {
    let plausible = key.len() >= 16
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if key.is_empty() || plausible {
        Ok(())
    } else {
        Err(
            "THAT IS NOT A KEY: 16 OR MORE LETTERS AND DIGITS, NO SPACES. PASTE IT WITH CTRL+V."
                .into(),
        )
    }
}

/// "ba, af  STAR_ALLIANCE" -> ["BA", "AF", "STAR_ALLIANCE"]
fn parse_airlines(text: &str) -> Result<Vec<String>, String> {
    const ALLIANCES: [&str; 3] = ["STAR_ALLIANCE", "SKYTEAM", "ONEWORLD"];
    let mut out = Vec::new();
    for raw in text
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
    {
        let code = raw.to_ascii_uppercase();
        let valid = ALLIANCES.contains(&code.as_str())
            || (code.len() == 2 && code.bytes().all(|b| b.is_ascii_alphanumeric()));
        if !valid {
            return Err(format!(
                "'{raw}' IS NOT A 2-CHARACTER AIRLINE CODE OR AN ALLIANCE"
            ));
        }
        if !out.contains(&code) {
            out.push(code);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let opts = Options {
            key: None,
            demo: true,
            latency: false,
            ephemeral: true,
            charset: Some("rich".into()),
            dump: None,
        };
        let mut app = App::new(Config::default(), Session::default(), opts);
        // Tests must never read whatever the person at the keyboard copied.
        app.clipboard = || None;
        app
    }

    fn scripted(script: &str) -> App {
        let mut app = app();
        script.split_whitespace().for_each(|token| app.feed(token));
        app
    }

    #[test]
    fn airline_filter_parsing() {
        assert_eq!(
            parse_airlines("ba, af oneworld BA"),
            Ok(vec!["BA".into(), "AF".into(), "ONEWORLD".into()])
        );
        assert_eq!(parse_airlines(""), Ok(vec![]));
        assert!(parse_airlines("british").is_err());
    }

    #[test]
    fn scan_fills_the_board_and_selects_the_cheapest() {
        let app = scripted("from:LHR to:NRT s wait");
        assert_eq!(app.inflight, 0);
        assert_eq!(app.focus, Pane::Tracks);
        let tracks = app.tracks();
        assert!(tracks.len() >= 5);
        let prices: Vec<f64> = tracks.iter().filter_map(|it| it.price).collect();
        assert!(
            prices.windows(2).all(|w| w[0] <= w[1]),
            "default sort is by price"
        );
        assert_eq!(app.status.0, Level::Good);
        assert!(
            app.status.1.starts_with("SCAN COMPLETE"),
            "{}",
            app.status.1
        );
    }

    #[test]
    fn scan_without_a_route_explains_instead_of_searching() {
        let app = scripted("s wait");
        assert!(app.board.slots.is_empty());
        assert_eq!(app.status.0, Level::Alert);
        assert_eq!(app.focus, Pane::Form);
    }

    #[test]
    fn flexible_dates_scan_every_day_in_the_window() {
        let mut app = app();
        app.form.flex = 3;
        "from:JFK to:CDG s wait"
            .split_whitespace()
            .for_each(|t| app.feed(t));
        assert_eq!(app.board.slots.len(), 7);
        assert!(
            app.board
                .slots
                .values()
                .all(|s| matches!(s, Slot::Ready(_)))
        );
        // Every scanned date was filed in the fare log.
        assert_eq!(app.fare_map().unwrap().known.len(), 7);
        // Stepping the radar changes which date the table shows.
        let before = app.board.active;
        app.feed("]");
        assert_eq!(app.board.active, before + Duration::days(1));
    }

    #[test]
    fn as_soon_as_possible_from_tomorrow_never_searches_an_earlier_day() {
        let mut app = scripted("from:SYD to:PER");
        let today = Local::now().date_naive();
        let tomorrow = today + Duration::days(1);
        // Backspace on the date field means "the soonest": tomorrow.
        app.form.cursor = app
            .form
            .fields()
            .iter()
            .position(|f| *f == Field::Depart)
            .unwrap();
        app.feed("bksp");
        assert_eq!(app.form.depart, tomorrow);
        // Either-side flex reaches back to today, which is no use here...
        (app.form.flex, app.form.flex_later) = (3, false);
        assert!(app.scan_dates().contains(&today));
        // ...so step the flex field round to "or up to 7 later".
        app.form.cursor = app
            .form
            .fields()
            .iter()
            .position(|f| *f == Field::Flex)
            .unwrap();
        while (app.form.flex, app.form.flex_later) != (7, true) {
            app.feed("right");
        }
        let dates = app.scan_dates();
        assert_eq!(dates.len(), 8, "tomorrow and the seven days after");
        assert_eq!(
            dates.first(),
            Some(&tomorrow),
            "the day asked for is searched first"
        );
        assert!(dates.iter().all(|d| *d >= tomorrow), "{dates:?}");
        app.feed("s");
        app.feed("wait");
        assert_eq!(app.board.slots.len(), 8);
        assert_eq!(app.board.slots.keys().next(), Some(&tomorrow));
        // Every choice the field offers survives being saved and restored.
        for (flex, later) in FLEX_CHOICES {
            (app.form.flex, app.form.flex_later) = (flex, later);
            let restored = Form::from_session(&app.form.to_session(), today);
            assert_eq!((restored.flex, restored.flex_later), (flex, later));
        }
    }

    #[test]
    fn round_trip_walks_outbound_then_return() {
        let mut app = app();
        app.form.round_trip = true;
        "from:LHR to:JFK s wait"
            .split_whitespace()
            .for_each(|t| app.feed(t));
        let outbound_price = app.selected().unwrap().price;
        app.feed("enter");
        assert!(
            app.board.ret.is_some(),
            "Enter on an outbound fetches return legs"
        );
        app.feed("wait");
        let back = app.selected().expect("return options loaded");
        assert_eq!((back.origin(), back.dest()), ("JFK", "LHR"));
        assert_eq!(
            back.price, outbound_price,
            "cheapest return matches the quoted total"
        );
        app.feed("enter");
        assert!(matches!(app.popup, Some(Popup::Dossier { .. })));
        app.feed("esc");
        app.feed("esc");
        assert!(
            app.board.ret.is_none(),
            "Esc backs out to the outbound list"
        );
    }

    #[test]
    fn airport_picker_types_filters_and_sets_the_field() {
        let mut app = app();
        app.feed("down"); // TRIP -> FROM
        app.feed("enter");
        app.feed("text:tokyo");
        match &app.popup {
            Some(Popup::Airport { hits, .. }) => assert!(hits.iter().any(|a| a.iata == "HND")),
            _ => panic!("airport picker should be open"),
        }
        app.feed("enter");
        assert!(app.popup.is_none());
        assert_eq!(app.form.origin.map(|a| a.city), Some("Tokyo"));
    }

    #[test]
    fn time_window_editor_validates() {
        let mut app = app();
        app.form.cursor = app
            .form
            .fields()
            .iter()
            .position(|f| *f == Field::DepWindow)
            .unwrap();
        app.feed("enter");
        app.feed("text:0700-9999");
        app.feed("enter");
        assert!(
            matches!(&app.popup, Some(Popup::Text { error: Some(_), .. })),
            "bad input keeps the editor open"
        );
        for _ in 0..9 {
            app.feed("bksp");
        }
        app.feed("text:0700-1130");
        app.feed("enter");
        assert!(app.popup.is_none());
        assert_eq!(app.form.dep_window, TimeWindow(Some((420, 690))));
    }

    #[test]
    fn departure_never_moves_into_the_past_and_drags_the_return_with_it() {
        let mut app = app();
        let today = Local::now().date_naive();
        let length = app.form.ret - app.form.depart;
        app.set_depart(today - Duration::days(10));
        assert_eq!(app.form.depart, today);
        assert_eq!(app.form.ret - app.form.depart, length);
        app.set_return(today - Duration::days(3));
        assert_eq!(app.form.ret, app.form.depart);
    }

    #[test]
    fn a_survey_prices_every_day_and_measures_them_all() {
        let mut app = scripted("from:LHR to:NRT");
        let today = Local::now().date_naive();
        let ranges = app.survey_ranges(2);
        assert_eq!(
            ranges.len(),
            3,
            "six months is three calendars of two months"
        );
        app.launch_survey(2);
        assert_eq!(app.survey_left, 3);
        app.settle();
        assert_eq!(app.survey_left, 0);
        assert!(
            app.status.1.starts_with("SURVEY COMPLETE: 183 DAYS PRICED"),
            "{}",
            app.status.1
        );
        let map = app.fare_map().unwrap();
        assert_eq!(
            map.known.len(),
            183,
            "today and each of the 182 days after it"
        );
        // Nothing is estimated: any day in range is a fare that was seen.
        for offset in [0, 1, 60, 61, 100, 182] {
            assert!(
                matches!(map.at(today + Duration::days(offset)), Some((_, true))),
                "day {offset}"
            );
        }
        // A second survey has nothing left to ask for.
        assert!(app.survey_ranges(2).is_empty());
        app.launch_survey(2);
        assert_eq!(app.survey_left, 0);
        assert!(app.status.1.contains("UP TO DATE"), "{}", app.status.1);
    }

    #[test]
    fn a_year_is_six_calls_that_meet_end_to_end() {
        let app = scripted("from:LHR to:NRT");
        let today = Local::now().date_naive();
        for (horizon, calls) in [(0, 1), (1, 2), (2, 3), (YEAR, 6)] {
            let ranges = app.survey_ranges(horizon);
            // Cheap enough to do on a free plan's 250 searches a month.
            assert_eq!(ranges.len(), calls, "{}", HORIZONS[horizon].0);
            assert_eq!(ranges[0].0, today);
            assert_eq!(
                ranges[calls - 1].1,
                today + Duration::days(HORIZONS[horizon].1)
            );
            for (from, to) in &ranges {
                assert!(
                    (*to - *from).num_days() < CALENDAR_DAYS,
                    "no call asks for more than it may"
                );
            }
            for pair in ranges.windows(2) {
                assert_eq!(
                    pair[1].0,
                    pair[0].1 + Duration::days(1),
                    "no day missed, none asked twice"
                );
            }
        }
    }

    #[test]
    fn a_scan_s_few_dates_do_not_pass_for_a_surveyed_range() {
        let app = scripted("from:LHR to:NRT s wait");
        assert!(
            app.fare_map().unwrap().known.len() >= 4,
            "the scan logged its dates"
        );
        assert_eq!(
            app.survey_ranges(YEAR).len(),
            6,
            "and the survey still asks for every range"
        );
    }

    #[test]
    fn a_survey_of_a_route_to_itself_is_refused_before_it_costs_anything() {
        let mut app = scripted("from:LHR to:LHR");
        app.launch_survey(YEAR);
        assert_eq!(app.survey_left, 0);
        assert_eq!(app.status.0, Level::Alert);
    }

    #[test]
    fn opening_intel_surveys_the_year_when_that_is_free() {
        // A scan first: its handful of dates must not pass for a year's data.
        let mut app = scripted("from:SYD to:SGN s wait i wait");
        assert!(app.intel.is_some());
        assert!(app.popup.is_none(), "the simulator does not need to ask");
        let map = app.fare_map().unwrap();
        assert_eq!(map.known.len(), 365, "every day of the year ahead");
        // With that much data every month has a best and a worst day.
        let today = Local::now().date_naive();
        let next_month = today.with_day(1).unwrap() + Months::new(1);
        let ((low_day, low), (high_day, high)) = map.month_extremes(next_month, today).unwrap();
        assert!(low < high && low_day != high_day);
        // Closing and reopening does not survey again.
        app.feed("i");
        app.feed("i");
        assert_eq!(app.survey_left, 0);
    }

    #[test]
    fn sorting_twice_by_the_same_column_reverses_it() {
        let mut app = scripted("from:LHR to:NRT s wait");
        let prices = |app: &App| {
            app.tracks()
                .iter()
                .filter_map(|it| it.price)
                .collect::<Vec<f64>>()
        };
        let ascending = prices(&app);
        app.set_sort(Sort::Price);
        assert!(app.board.descending);
        let descending = prices(&app);
        assert_eq!(
            descending,
            ascending.iter().rev().copied().collect::<Vec<_>>()
        );
        // A different column starts ascending again.
        app.feed("6");
        assert_eq!((app.board.sort, app.board.descending), (Sort::Stops, false));
        let stops: Vec<usize> = app.tracks().iter().map(|it| it.stops()).collect();
        assert!(stops.windows(2).all(|w| w[0] <= w[1]), "{stops:?}");
        app.feed("5");
        let arrivals: Vec<_> = app.tracks().iter().map(|it| it.arr()).collect();
        assert!(arrivals.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn changing_currency_requotes_rather_than_relabels() {
        let mut app = scripted("from:SYD to:SGN s wait");
        assert_eq!(app.cfg.currency, "AUD");
        let in_aud = app.selected().unwrap().price.unwrap();
        // Type a name into the picker and take the first match.
        app.feed("$");
        app.feed("text:yen");
        app.feed("enter");
        assert_eq!(app.cfg.currency, "JPY");
        assert!(app.popup.is_none());
        app.settle();
        let in_jpy = app.selected().unwrap().price.unwrap();
        assert!(
            in_jpy > in_aud * 50.0,
            "A${in_aud} must not be shown as ¥{in_jpy}"
        );
        assert_eq!(
            app.searched.as_ref().map(|q| q.currency.as_str()),
            Some("JPY")
        );
        // A code that is not in the list can still be typed in full.
        app.feed("$");
        app.feed("text:isk");
        app.feed("enter");
        assert_eq!(app.cfg.currency, "ISK");
        // Nonsense leaves the picker open and the currency alone.
        app.feed("$");
        app.feed("text:zz");
        app.feed("enter");
        assert!(matches!(app.popup, Some(Popup::Currency { .. })));
        assert_eq!(app.cfg.currency, "ISK");
    }

    #[test]
    fn a_place_keeps_its_colour_on_the_way_out_and_the_way_back() {
        let mut app = app();
        app.form.round_trip = true;
        "from:LHR to:SYD s wait"
            .split_whitespace()
            .for_each(|t| app.feed(t));
        let out = app.routes();
        assert_eq!(
            out.primary.first().map(|leg| leg.t0),
            Some(0.0),
            "the trip starts at the cyan end"
        );
        assert_eq!(
            out.primary.last().map(|leg| leg.t1),
            Some(1.0),
            "and ends at the pink end"
        );
        assert!(
            out.primary.windows(2).all(|w| w[0].t1 == w[1].t0),
            "each flight picks up where the last ended"
        );
        let tone_of =
            |routes: &Routes, kind| routes.marks.iter().find(|m| m.kind == kind).map(|m| m.tone);
        assert_eq!(tone_of(&out, MarkKind::Origin), Some(0.0));
        assert_eq!(tone_of(&out, MarkKind::Dest), Some(1.0));
        // Choosing a return: the flight in hand now runs pink to cyan.
        app.feed("enter");
        app.feed("wait");
        let back = app.routes();
        assert_eq!(back.primary.first().map(|leg| leg.t0), Some(1.0));
        assert_eq!(back.primary.last().map(|leg| leg.t1), Some(0.0));
        assert_eq!(
            back.secondary.first().map(|leg| leg.t0),
            Some(0.0),
            "the locked outbound still runs forward"
        );
    }

    #[test]
    fn switching_data_source_clears_results() {
        let mut app = scripted("from:LHR to:NRT s wait");
        assert!(!app.board.slots.is_empty());
        app.install_key("");
        assert!(app.board.slots.is_empty());
        assert!(!app.live);
        assert_eq!(app.key_state, KeyState::Absent);
    }

    // ---- settings ----------------------------------------------------------
    // (The TEXT SIZE row is never stepped here: on a real console it would
    // change the font of whoever is running the tests.)

    #[test]
    fn settings_open_with_f2_and_change_from_the_keyboard() {
        let mut app = scripted("f2");
        assert!(matches!(app.popup, Some(Popup::Settings { sel: 0 })));
        let theme = app.cfg.theme.clone();
        "down down right"
            .split_whitespace()
            .for_each(|t| app.feed(t));
        assert_ne!(app.cfg.theme, theme);
        assert!(
            matches!(app.popup, Some(Popup::Settings { sel: 2 })),
            "changing a value keeps the screen open"
        );
        // SYMBOLS steps through its three choices and comes back round.
        "down left".split_whitespace().for_each(|t| app.feed(t));
        assert_eq!(app.cfg.charset, "safe");
        app.feed("right");
        assert_eq!(app.cfg.charset, "auto");
        // CONTINENTS and BORDERS are switches; Enter flips one too.
        "down enter down left"
            .split_whitespace()
            .for_each(|t| app.feed(t));
        assert!(!app.cfg.continents && !app.cfg.borders);
        // KEEP RESULTS steps along fixed choices, 6 hours to 12.
        "down down right"
            .split_whitespace()
            .for_each(|t| app.feed(t));
        assert_eq!(app.cfg.cache_hours, 12);
        // The list stops at its ends, and Esc closes it for good.
        "down down".split_whitespace().for_each(|t| app.feed(t));
        assert!(matches!(app.popup, Some(Popup::Settings { sel: 7 })));
        app.feed("esc");
        assert!(app.popup.is_none());
    }

    #[test]
    fn a_picker_opened_from_settings_hands_back_to_settings() {
        let mut app = scripted("f2 down enter");
        assert!(matches!(app.popup, Some(Popup::Currency { .. })));
        "text:nzd enter"
            .split_whitespace()
            .for_each(|t| app.feed(t));
        assert_eq!(app.cfg.currency, "NZD");
        assert!(matches!(app.popup, Some(Popup::Settings { sel: 1 })));
        // Backing out of a picker returns there too.
        "enter esc".split_whitespace().for_each(|t| app.feed(t));
        assert!(matches!(app.popup, Some(Popup::Settings { sel: 1 })));
        app.feed("esc");
        assert!(app.popup.is_none());
        // Opened from the top bar instead, the picker just closes.
        "$ esc".split_whitespace().for_each(|t| app.feed(t));
        assert!(app.popup.is_none());
    }

    #[test]
    fn ctrl_v_pastes_the_clipboard_and_never_types_a_v() {
        let mut app = scripted("f2 enter");
        let typed = |app: &App| match &app.popup {
            Some(Popup::Text {
                kind: TextKind::ApiKey,
                input,
                ..
            }) => input.clone(),
            _ => panic!("the key box should be open"),
        };
        let ctrl_v = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL);
        app.on_key(ctrl_v);
        assert_eq!(
            typed(&app),
            "",
            "an empty clipboard types nothing, least of all a v"
        );
        app.clipboard = || Some("abc 123\r\n".to_string());
        app.on_key(ctrl_v);
        assert_eq!(typed(&app), "abc 123", "line ends are dropped");
        // Shift+Insert and a right-click are the same paste.
        app.on_key(KeyEvent::new(KeyCode::Insert, KeyModifiers::SHIFT));
        let right_click = MouseEventKind::Down(MouseButton::Right);
        app.on_mouse(MouseEvent {
            kind: right_click,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(typed(&app), "abc 123abc 123abc 123");
        // What was pasted is not a key, and is refused before it is stored.
        app.feed("enter");
        assert!(matches!(
            &app.popup,
            Some(Popup::Text { error: Some(_), .. })
        ));
        assert_eq!(app.cfg.api_key, "");
        assert!(!app.live);
        // Giving up on the key box lands on the settings screen it leads to.
        app.feed("esc");
        assert!(matches!(app.popup, Some(Popup::Settings { sel: 0 })));
    }

    #[test]
    fn a_ctrl_chord_is_never_taken_for_its_letter() {
        let mut app = scripted("from:LHR to:NRT");
        app.on_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert!(app.board.slots.is_empty(), "Ctrl+S is not a scan");
        app.on_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL));
        assert!(app.popup.is_none(), "and Ctrl+V is not a survey");
        // AltGr arrives as Ctrl+Alt, and types an ordinary character.
        app.feed("K");
        app.on_key(KeyEvent::new(
            KeyCode::Char('@'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        ));
        assert!(matches!(&app.popup, Some(Popup::Text { input, .. }) if input == "@"));
    }

    #[test]
    fn key_text_must_look_like_a_key() {
        assert!(check_key_text("").is_ok(), "empty means: remove the key");
        assert!(
            check_key_text("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
                .is_ok()
        );
        assert!(check_key_text("short").is_err());
        assert!(
            check_key_text("0123456789abcdef 0123456789abcdef").is_err(),
            "a space is a slip of the paste"
        );
        assert_eq!(tail("0123456789abcdef"), "cdef");
        assert_eq!(tail("abc"), "abc");
        assert_eq!(tail(""), "");
    }

    #[test]
    fn the_account_s_answer_says_whether_the_key_is_good() {
        let mut app = app();
        // As if a key had just been stored. (No test goes near the network.)
        app.live = true;
        app.key_state = KeyState::Checking;
        let fuel = Quota {
            left: 244,
            per_month: 250,
        };
        app.on_quota(Ok(Some(fuel)));
        assert_eq!(app.key_state, KeyState::Accepted);
        assert_eq!(app.quota, Some(fuel));
        assert!(
            app.status.1.contains("KEY ACCEPTED") && app.status.1.contains("244 OF 250"),
            "{}",
            app.status.1
        );
        // The routine look at the fuel gauge after a scan does not interrupt.
        app.set_status(Level::Info, "SCANNING");
        app.on_quota(Ok(Some(Quota {
            left: 243,
            per_month: 250,
        })));
        assert_eq!(app.status.1, "SCANNING");
        app.on_quota(Err(ProviderError::Network("offline".into())));
        assert_eq!(
            app.key_state,
            KeyState::Accepted,
            "a dropped connection does not unmake a good key"
        );
        assert_eq!(app.status.1, "SCANNING");
        // A rejection always speaks up.
        app.on_quota(Err(ProviderError::Auth("Invalid API key.".into())));
        assert!(matches!(app.key_state, KeyState::Rejected(_)));
        assert_eq!(app.status.0, Level::Alert);
        // Asked for and unreachable: neither accepted nor rejected.
        app.key_state = KeyState::Checking;
        app.on_quota(Err(ProviderError::Network("offline".into())));
        assert!(matches!(app.key_state, KeyState::Unchecked(_)));
        // With no key in use, a late answer about an old one is ignored.
        app.install_key("");
        app.on_quota(Ok(Some(fuel)));
        assert_eq!((app.key_state.clone(), app.quota), (KeyState::Absent, None));
    }

    #[test]
    fn saving_from_settings_says_what_happened() {
        let app = scripted("f2 s");
        assert!(
            app.status.1.contains("NOTHING WRITTEN"),
            "a scripted run never touches the file: {}",
            app.status.1
        );
        assert!(app.saved.is_none());
        assert!(
            matches!(app.popup, Some(Popup::Settings { .. })),
            "and the screen stays open"
        );
    }

    #[test]
    fn a_failed_piece_of_a_survey_is_reported_not_swallowed() {
        let mut app = scripted("from:LHR to:NRT");
        app.survey_left = 2;
        let route = app.route_key().unwrap();
        let day = Local::now().date_naive() + Duration::days(9);
        app.on_calendar(app.generation, route.clone(), Ok(vec![(day, 512.0)]));
        assert_eq!(app.survey_left, 1);
        app.on_calendar(
            app.generation,
            route.clone(),
            Err(ProviderError::Api("upstream timeout".into())),
        );
        assert_eq!(app.survey_left, 0);
        assert_eq!(app.status.0, Level::Alert);
        assert!(
            app.status.1.contains("UPSTREAM TIMEOUT") || app.status.1.contains("upstream timeout")
        );
        // What did arrive is kept.
        assert_eq!(app.fare_map().unwrap().known[&day].price, 512.0);
        // An answer to a scan since replaced is dropped whole.
        app.on_calendar(
            app.generation + 1,
            route,
            Ok(vec![(day + Duration::days(1), 1.0)]),
        );
        assert_eq!(app.fare_map().unwrap().known.len(), 1);
    }
}
