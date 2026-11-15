//! icarus-search: a retro command-terminal for hunting cheap flights.

mod airports;
mod app;
mod cache;
mod config;
mod console;
mod continents;
mod currency;
mod farelog;
mod fmt;
mod geo;
mod geodata;
mod model;
mod provider;
mod score;
mod ui;
mod worker;

use std::{
    io::{self, IsTerminal, stdout},
    path::PathBuf,
    process::ExitCode,
    time::{Duration, Instant},
};

use anyhow::Context;
use ratatui::{
    Terminal,
    backend::{CrosstermBackend, TestBackend},
    crossterm::{
        event::{
            self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste,
            EnableMouseCapture, Event, KeyEventKind, MouseEvent, MouseEventKind,
        },
        execute,
        terminal::{
            EnterAlternateScreen, LeaveAlternateScreen, SetTitle, disable_raw_mode, enable_raw_mode,
        },
    },
    layout::Position,
};

use app::{App, Level, Options};
use config::{Config, KeySource, Session};
use model::{Cabin, Query, Stage, Stops, TimeWindow};
use provider::{CALENDAR_DAYS, Provider, ProviderError, demo::Demo, serpapi::SerpApi};

const USAGE: &str = "\
icarus-search - hunt cheap flights from a retro command terminal

USAGE:
    icarus-search [OPTIONS]

OPTIONS:
    --demo               Use simulated fares even if an API key is configured
    --key <KEY>          SerpApi key for this run (overrides env and config)
    --charset <MODE>     rich | safe | auto. 'safe' avoids braille and other
                         symbols that older terminal fonts cannot draw
    --text-size <PX>     Text height on the classic Windows console. Smaller
                         text fits more characters, so a finer map
    -h, --help           Show this help
    -V, --version        Show the version

ENVIRONMENT:
    ICARUS_API_KEY       SerpApi key (overrides the config file)

Inside the app, press ? for the key reference and F2 for the settings.

SETTINGS:
    icarus-search.toml, beside the program: currency, theme, key and the
    last search. Created on first run; change it from the app (F2) or by
    hand while the program is closed.

DIAGNOSTICS:
    --probe <FROM-TO>    Check the live fare source without opening the app:
                         one search and one price calendar for the route,
                         e.g. --probe SYD-PER. Spends 2 searches. With
                         --demo it checks against the simulator instead
    --dump <DIR>         Keep every raw reply from the live source in DIR
    --snapshot <WxH>     Render one frame of the simulator as text and exit
    --keys \"<script>\"    Scripted input applied at start-up (or before a
                         snapshot), e.g. \"from:SYD to:SGN s wait\"
    --bench              With --snapshot: also time the renderer
";

#[derive(Default)]
struct Args {
    demo: bool,
    key: Option<String>,
    charset: Option<String>,
    text_size: Option<u16>,
    probe: Option<String>,
    dump: Option<PathBuf>,
    snapshot: Option<(u16, u16)>,
    keys: String,
    bench: bool,
    help: bool,
    version: bool,
}

fn parse_args(mut argv: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut args = Args::default();
    while let Some(flag) = argv.next() {
        let mut value = |name: &str| argv.next().ok_or(format!("{name} needs a value"));
        match flag.as_str() {
            "--demo" => args.demo = true,
            "--bench" => args.bench = true,
            "-h" | "--help" => args.help = true,
            "-V" | "--version" => args.version = true,
            "--key" => args.key = Some(value("--key")?),
            "--keys" => args.keys = value("--keys")?,
            "--probe" => args.probe = Some(value("--probe")?),
            "--dump" => args.dump = Some(PathBuf::from(value("--dump")?)),
            "--charset" => {
                let mode = value("--charset")?.to_ascii_lowercase();
                if !["rich", "safe", "auto"].contains(&mode.as_str()) {
                    return Err(format!(
                        "--charset must be rich, safe or auto, not '{mode}'"
                    ));
                }
                args.charset = Some(mode);
            }
            "--text-size" => {
                let size = value("--text-size")?;
                let pixels = size.parse::<u16>().ok().filter(|p| (6..=72).contains(p));
                args.text_size =
                    Some(pixels.ok_or(format!("--text-size wants 6 to 72 pixels, not '{size}'"))?);
            }
            "--snapshot" => {
                let size = value("--snapshot")?;
                let parsed = size
                    .to_ascii_lowercase()
                    .split_once('x')
                    .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)));
                args.snapshot =
                    Some(parsed.ok_or(format!("--snapshot wants WIDTHxHEIGHT, not '{size}'"))?);
            }
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    Ok(args)
}

fn main() -> ExitCode {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("icarus-search: {message}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    if args.help {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if args.version {
        println!("icarus-search {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    let outcome = match (&args.probe, args.snapshot) {
        (Some(route), _) => probe(&args, route),
        (None, Some(size)) => snapshot(&args, size),
        (None, None) => interactive(&args),
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("icarus-search: {error:#}");
            ExitCode::FAILURE
        }
    }
}

// ------------------------------------------------------------- interactive

/// Redraws triggered by input are capped at about 60 a second.
const FRAME: Duration = Duration::from_millis(16);
/// Animation rate while something is happening...
const TICK_ACTIVE: Duration = Duration::from_millis(50);
/// ...and while nothing is, to go easy on laptop batteries.
const TICK_IDLE: Duration = Duration::from_millis(125);

fn interactive(args: &Args) -> anyhow::Result<()> {
    // Piped or redirected, there is nobody to draw for and no keyboard to
    // read: say so and stop, rather than run invisibly for ever.
    if !stdout().is_terminal() || !io::stdin().is_terminal() {
        anyhow::bail!(
            "this is an interactive program and needs a terminal window. \
             Run it directly (not piped or redirected), or use --snapshot for text output."
        );
    }
    let (cfg, warning) = Config::load();
    // Find out what this console can draw before anything is drawn. On the
    // classic Windows console this may borrow a better font for the session.
    let wanted = args.charset.clone().unwrap_or_else(|| cfg.charset.clone());
    let charset = console::prepare(&wanted, args.text_size.unwrap_or(cfg.text_size));
    let opts = Options {
        key: args.key.clone(),
        demo: args.demo,
        // A scripted start is a demonstration: it skips the simulator's
        // pretend network delay, and leaves the settings file, fare log
        // and cache exactly as it found them.
        latency: args.keys.is_empty(),
        ephemeral: !args.keys.is_empty(),
        charset: Some(charset.into()),
        dump: args.dump.clone(),
    };
    let mission = cfg.mission.clone();
    let mut app = App::new(cfg, mission, opts);
    // Write the settings straight away, so that from the first run there is
    // a file beside the program to find and edit.
    app.persist();
    if let Some(warning) = warning {
        app.set_status(Level::Alert, warning);
    }
    for token in args.keys.split_whitespace() {
        app.feed(token);
    }
    let mut terminal = enter_terminal().context("could not take over the terminal")?;
    let result = event_loop(&mut terminal, &mut app);
    leave_terminal();
    app.persist();
    result.context("terminal I/O failed")
}

fn enter_terminal() -> io::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    // If anything panics, give the user their terminal back before the
    // message prints, or it would be lost in the alternate screen.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        leave_terminal();
        default_hook(info);
    }));
    enable_raw_mode()?;
    execute!(
        stdout(),
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste,
        SetTitle("ICARUS-SEARCH")
    )?;
    Terminal::new(CrosstermBackend::new(stdout()))
}

fn leave_terminal() {
    let _ = execute!(
        stdout(),
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen
    );
    let _ = disable_raw_mode();
    console::restore();
}

/// The loop is built around one rule: input is never left waiting.
///
/// - Every queued event is handled before the next frame is drawn.
/// - A frame is drawn as soon as input arrives, not on the next timer tick.
/// - The timer only drives animation, and slows down when the app is idle.
fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> io::Result<()> {
    let mut last_tick = Instant::now();
    let mut last_draw: Option<Instant> = None;
    let mut dirty = true;
    while !app.quit {
        let since_draw = last_draw.map_or(FRAME, |at| at.elapsed());
        if dirty && since_draw >= FRAME {
            terminal.draw(|frame| ui::draw(frame, app))?;
            last_draw = Some(Instant::now());
            dirty = false;
        }
        let tick = if app.busy() { TICK_ACTIVE } else { TICK_IDLE };
        let mut wait = tick.saturating_sub(last_tick.elapsed());
        if dirty {
            // A redraw is owed: wake as soon as the frame cap allows.
            wait = wait.min(FRAME.saturating_sub(last_draw.map_or(FRAME, |at| at.elapsed())));
        }
        if event::poll(wait)? {
            pump(app)?;
            dirty = true;
        }
        let elapsed = last_tick.elapsed();
        if elapsed >= tick {
            // Cap the step so a suspended laptop does not fast-forward.
            app.on_tick(elapsed.as_secs_f64().min(0.25));
            last_tick = Instant::now();
            dirty = true;
        }
    }
    Ok(())
}

/// Handle everything in the input queue, coalescing the floods: of many
/// pointer moves only the last position matters, and wheel ticks add up to
/// one net step. Without this, a sweep of the mouse queues hundreds of
/// events and the interface falls seconds behind the hand.
fn pump(app: &mut App) -> io::Result<()> {
    let mut moved: Option<MouseEvent> = None;
    let mut wheel: Option<(Position, i32)> = None;
    loop {
        match event::read()? {
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::Moved => moved = Some(mouse),
                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                    let step = if mouse.kind == MouseEventKind::ScrollUp {
                        -1
                    } else {
                        1
                    };
                    let so_far = wheel.map_or(0, |(_, steps)| steps);
                    wheel = Some((Position::new(mouse.column, mouse.row), so_far + step));
                }
                _ => {
                    // Clicks and drags must see earlier movement applied first.
                    flush(app, &mut moved, &mut wheel);
                    app.on_mouse(mouse);
                }
            },
            // Windows reports key releases too; acting on them would make
            // every key fire twice.
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                flush(app, &mut moved, &mut wheel);
                app.on_key(key);
            }
            Event::Paste(text) => {
                flush(app, &mut moved, &mut wheel);
                app.on_paste(text);
            }
            // Resizes and focus changes need only the redraw that follows.
            _ => {}
        }
        if !event::poll(Duration::ZERO)? {
            break;
        }
    }
    flush(app, &mut moved, &mut wheel);
    Ok(())
}

fn flush(app: &mut App, moved: &mut Option<MouseEvent>, wheel: &mut Option<(Position, i32)>) {
    if let Some((position, steps)) = wheel.take() {
        app.on_scroll(position, steps);
    }
    if let Some(mouse) = moved.take() {
        app.on_mouse(mouse);
    }
}

// ------------------------------------------------------------------- probe

/// Ask the fare source two real questions and report what came back: is the
/// key accepted, does a search parse, does the price calendar answer, and
/// do the two agree on what the same day costs.
///
/// It is the quickest way to tell "this program is broken" from "the
/// service has changed", and the one check of the live path that needs
/// nobody at the keyboard. The report goes to stdout; a failure also ends
/// with a non-zero exit code, so a script can act on it.
fn probe(args: &Args, route: &str) -> anyhow::Result<()> {
    let (from, to) = route
        .split_once(['-', '>', ':'])
        .with_context(|| format!("--probe wants a route like SYD-PER, not '{route}'"))?;
    let airport = |code: &str| {
        let code = code.trim().to_ascii_uppercase();
        airports::db()
            .get(&code)
            .map(|a| a.iata.to_string())
            .with_context(|| format!("'{code}' is not an airport code this program knows"))
    };
    let (origin, dest) = (airport(from)?, airport(to)?);
    let (cfg, _) = Config::load();
    let (mut source, described): (Box<dyn Provider>, String) = if args.demo {
        (
            Box::new(Demo::new(false)),
            "the simulator (--demo): this checks the program, not the service".into(),
        )
    } else {
        let (key, found) = config::resolve_key(args.key.as_deref(), &cfg).context(
            "no API key to check. Add one in the app (F2), in icarus-search.toml, \
             with --key or in ICARUS_API_KEY - or add --demo to run this against the simulator",
        )?;
        let place = match found {
            KeySource::Flag => "--key",
            KeySource::Env => "ICARUS_API_KEY",
            KeySource::File => "the settings file",
            KeySource::Baked => "this build",
        };
        (
            Box::new(SerpApi::new(key, args.dump.clone())),
            format!("SerpApi, with the key from {place}"),
        )
    };
    let live = source.is_live();
    let depart = chrono::Local::now().date_naive() + chrono::Duration::days(30);
    let currency = cfg.currency.clone();
    let query = Query {
        origin,
        dest,
        depart,
        ret: None,
        adults: 1,
        cabin: Cabin::Economy,
        stops: Stops::Any,
        airlines: Vec::new(),
        dep_window: TimeWindow::ANY,
        arr_window: TimeWindow::ANY,
        currency: currency.clone(),
    };
    let money = |value: f64| fmt::money(value, &currency);
    println!("icarus-search live check");
    println!(
        "  route      {} > {}, one way, {depart}, 1 adult, economy, fares in {currency}",
        query.origin, query.dest
    );
    println!("  source     {described}");

    let mut failed: Vec<&str> = Vec::new();
    match source.quota() {
        Ok(Some(fuel)) => println!(
            "  key        accepted: {} of {} searches left this month",
            fuel.left, fuel.per_month
        ),
        Ok(None) => println!("  key        none needed"),
        Err(error) => {
            println!("  key        FAILED - {error}");
            failed.push("key");
            // A key the service refuses will be refused twice more; stop
            // before spending anything on finding that out.
            if matches!(error, ProviderError::Auth(_)) {
                anyhow::bail!("the key was rejected, so nothing else was tried");
            }
        }
    }

    let started = Instant::now();
    let mut searched_low = None;
    match source.search(&query, &Stage::Outbound) {
        Ok(found) => {
            let took = started.elapsed().as_secs_f64();
            searched_low = found.lowest_price();
            let cheapest = found
                .itineraries
                .iter()
                .filter(|it| it.price.is_some())
                .min_by(|a, b| {
                    a.price
                        .unwrap_or(f64::MAX)
                        .total_cmp(&b.price.unwrap_or(f64::MAX))
                });
            match cheapest {
                Some(it) => println!(
                    "  search     ok in {took:.1}s: {} flights, cheapest {} ({}, {}, {})",
                    found.itineraries.len(),
                    money(it.price.unwrap_or(0.0)),
                    it.carriers().join(" + "),
                    match it.stops() {
                        0 => "nonstop".to_string(),
                        1 => "1 stop".to_string(),
                        n => format!("{n} stops"),
                    },
                    fmt::duration(it.total_min).to_lowercase(),
                ),
                None => println!(
                    "  search     ok in {took:.1}s, but no priced flights came back for that day"
                ),
            }
            match found
                .insights
                .as_ref()
                .and_then(|i| i.typical.map(|range| (range, i.history.len())))
            {
                Some(((low, high), days)) => {
                    println!(
                        "  insights   usual range {} to {}, {days} days of price history",
                        money(low),
                        money(high)
                    )
                }
                None => {
                    println!("  insights   none in the reply (the usual-range band will be empty)")
                }
            }
        }
        Err(error) => {
            println!("  search     FAILED - {error}");
            failed.push("search");
        }
    }

    let started = Instant::now();
    let until = depart + chrono::Duration::days(CALENDAR_DAYS - 1);
    match source.calendar(&query, depart, until) {
        Ok(fares) => {
            let took = started.elapsed().as_secs_f64();
            match (
                fares.first(),
                fares.last(),
                fares.iter().min_by(|a, b| a.1.total_cmp(&b.1)),
            ) {
                (Some((first, _)), Some((last, _)), Some((best_day, best))) => {
                    println!(
                        "  calendar   ok in {took:.1}s: {} of the {CALENDAR_DAYS} days asked for are priced, {first} to {last}",
                        fares.len()
                    );
                    println!("             cheapest {} on {best_day}", money(*best));
                    // The heatmap is drawn from calendar fares and the flight
                    // list from searches: worth knowing whether they match.
                    match (fares.iter().find(|(day, _)| *day == depart), searched_low) {
                        (Some((_, quoted)), Some(listed)) if (quoted - listed).abs() < 0.5 => {
                            println!(
                                "  agreement  calendar and search both say {} for {depart}",
                                money(listed)
                            )
                        }
                        (Some((_, quoted)), Some(listed)) => println!(
                            "  agreement  for {depart} the calendar says {} and the search says {}: they differ",
                            money(*quoted),
                            money(listed)
                        ),
                        _ => println!(
                            "  agreement  not comparable: one of the two has no fare for {depart}"
                        ),
                    }
                }
                _ => println!(
                    "  calendar   ok in {took:.1}s, but it priced none of the {CALENDAR_DAYS} days asked for"
                ),
            }
        }
        Err(error) => {
            println!("  calendar   FAILED - {error}");
            failed.push("calendar");
        }
    }

    if live {
        if let Ok(Some(fuel)) = source.quota() {
            println!(
                "  fuel       {} of {} searches left after this check",
                fuel.left, fuel.per_month
            );
        }
        if let Some(dir) = &args.dump {
            println!("  replies    kept in {}", dir.display());
        }
    }
    if failed.is_empty() {
        println!("all good.");
        Ok(())
    } else {
        anyhow::bail!("the live check failed at: {}", failed.join(", "))
    }
}

// ---------------------------------------------------------------- snapshot

/// Render one frame of the simulator to stdout as plain text. This is how
/// the UI is checked without a human at an interactive terminal.
fn snapshot(args: &Args, (width, height): (u16, u16)) -> anyhow::Result<()> {
    let opts = Options {
        key: None,
        demo: true,
        latency: false,
        ephemeral: true,
        charset: Some(args.charset.clone().unwrap_or_else(|| "rich".into())),
        dump: None,
    };
    let mut app = App::new(Config::default(), Session::default(), opts);
    for token in args.keys.split_whitespace() {
        app.feed(token);
    }
    app.settle();
    let mut terminal = Terminal::new(TestBackend::new(width, height))?;
    terminal.draw(|frame| ui::draw(frame, &mut app))?;
    if args.bench {
        let frames = 200;
        let started = Instant::now();
        for _ in 0..frames {
            app.on_tick(0.05);
            terminal.draw(|frame| ui::draw(frame, &mut app))?;
        }
        let per_frame = started.elapsed().as_secs_f64() * 1000.0 / f64::from(frames);
        eprintln!("render: {per_frame:.2} ms/frame at {width}x{height}");
    }
    let buffer = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..height {
        let line: String = (0..width).map(|x| buffer[(x, y)].symbol()).collect();
        out.push_str(line.trim_end());
        out.push('\n');
    }
    print!("{out}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> Result<Args, String> {
        parse_args(line.split_whitespace().map(String::from))
    }

    #[test]
    fn flags_parse() {
        let args = parse("--demo --charset SAFE --snapshot 120x30 --key abc")
            .ok()
            .unwrap();
        assert!(args.demo);
        assert_eq!(args.charset.as_deref(), Some("safe"));
        assert_eq!(args.snapshot, Some((120, 30)));
        assert_eq!(args.key.as_deref(), Some("abc"));
    }

    #[test]
    fn bad_flags_are_rejected_with_a_reason() {
        assert!(parse("--wat").err().unwrap().contains("unknown option"));
        assert!(parse("--key").err().unwrap().contains("needs a value"));
        assert!(
            parse("--snapshot big")
                .err()
                .unwrap()
                .contains("WIDTHxHEIGHT")
        );
        assert!(
            parse("--charset emoji")
                .err()
                .unwrap()
                .contains("rich, safe or auto")
        );
        assert!(parse("--text-size 2").err().unwrap().contains("6 to 72"));
        assert_eq!(parse("--text-size 12").ok().unwrap().text_size, Some(12));
        assert!(parse("--probe").err().unwrap().contains("needs a value"));
    }

    #[test]
    fn the_live_check_runs_end_to_end_against_the_simulator() {
        let args = parse("--demo --probe SYD-PER --dump replies").ok().unwrap();
        assert_eq!(args.probe.as_deref(), Some("SYD-PER"));
        assert_eq!(args.dump, Some(PathBuf::from("replies")));
        // The simulator always answers, so every step of the check passes.
        assert!(
            probe(&args, "syd>per").is_ok(),
            "any of - > : separates the two codes, in any case"
        );
        // A route it cannot read is refused before anything is asked.
        let no_route = probe(&args, "SYDPER").err().unwrap().to_string();
        assert!(no_route.contains("SYD-PER"), "{no_route}");
        let no_airport = probe(&args, "SYD-QQQ").err().unwrap().to_string();
        assert!(no_airport.contains("QQQ"), "{no_airport}");
    }
}
