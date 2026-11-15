# icarus-search

A retro command-terminal for hunting cheap flights. One screen shows the
route on a spinning globe, every option ranked by price, each layover and its
wait, and whether today's fare is a good one.

```
 ICARUS-SEARCH  FLIGHT INTERCEPT TERMINAL                                 ● LIVE SERPAPI  │  FUEL 231/250  │  USD
┌ MISSION PARAMETERS ──────────────┐┌ GLOBE ───────────────────────────────────────────────────────────────────────────┐
│► TRIP     ROUND-TRIP             ││ ORTHO 2.6x        ⢀⡤⠊⠁⢉⡕⠰⠼⠚ ⢬⠉⠈⠉⠉⠉⠙⠋⠁      ⠡⠄⠤⠠⡀⣀ ⠈⠑⠨⠮⢲⢨⢆⡆⡀⠑⠤⡀9,591 KM  HDG 031° │
│  FROM     LHR LONDON             ││             ⡠⠊AMS⢴⣄LHR⠋⠉⠉⠉⠉⠈⠉⠉⠉⠉⠑⠒⠒⠤⠤⣀⡀⢦⣤⣀⢩⣷⡤⠶⠠⣴⠛⠈⠡⠡⢂⠡⠡⢰⣄⣗ ⠈⠁⢸⠂   ⠑⣄             │
│  TO       NRT NARITA             ││     ⢀⠄⠋⢹⡀⠡⠡⡸⠰⡀⠈⣁⠚⠉⠿⠦⠖⡉⠂⣀⠆⠥⣄⠓⢢⠂⡀⠡⠡⠁⠠⡠⡀⠉⠈⠂⣂⡀⠂⠡⠂⠂⠡⠡⠠⡀⠠⠡⡀⠙⢦⡀⡐   ⠙⢀⣄ NRT⣠⠎     ⠸⡀     │
│  DEPART   2026-12-14 MON         │└──────────────────────────────────────────────────────────────────────────────────┘
│  FLEX     ± 3 DAYS               │╔ TRACKS  OUTBOUND 2026-12-14 MON  14 FOUND  BY PRICE ═════════════════════════════╗
├ PRICE RADAR ─────────────────────┤║  #     PRICE  DEP   ARR     TIME    VIA / LAYOVER           CARRIER              ║
│ SUN 12-13   $1,007 █████████ HIGH│║  1      $733  0840  1848+1  25H08M  AMS 11H00  HND 1H10     HELIOS PACIFIC   LOW ║
│►MON 12-14     $733 █░░░░░░░░ LOW │║  2      $831  2045  2215+1  16H30M  FRA 2H35                ARIADNE EXPRESS  BEST║
│ TUE 12-15     $754 ██░░░░░░░     │║  4      $866  1800  1728+1  14H28M  HND 2H05                DD+IK                ║
```

## Get it running

1. Download the file for your computer from the
   [Releases](../../releases) page:

   | Your computer            | File                              |
   |--------------------------|-----------------------------------|
   | Windows                  | `icarus-search-windows-x64.exe`   |
   | Mac (Apple Silicon, M1+) | `icarus-search-macos-arm64`       |
   | Mac (Intel)              | `icarus-search-macos-intel`       |
   | Linux                    | `icarus-search-linux-x64`         |

2. Run it.
   - **Windows:** double-click it. Windows will say it "protected your PC"
     because the file is not signed by a company; choose *More info*, then
     *Run anyway*. It looks best in
     [Windows Terminal](https://aka.ms/terminal) (free from Microsoft).
   - **Mac / Linux:** open Terminal in the download folder and run:

     ```bash
     chmod +x icarus-search-* && ./icarus-search-*
     ```

     On a Mac, if it is blocked as "from an unidentified developer", run
     `xattr -d com.apple.quarantine icarus-search-*` once and try again.

3. Make the window big. It needs at least 96 x 24 characters and is at its
   best maximised.

It starts on **simulated fares** (the header says so, and blinks) so you can
learn the controls. Simulated prices are invented, on invented airlines. For
real prices, add a key, below.

## Real prices

Live fares come from Google Flights through [SerpApi](https://serpapi.com).
Their free plan gives 250 searches a month with no card.

1. Sign up at serpapi.com and copy your API key.
2. In icarus-search press **F2** (or click `F2 SETTINGS` in the top bar),
   press **Enter** on `API KEY`, paste the key with **Ctrl+V** or a
   right-click, and press **Enter**.

The key is checked straight away, which costs nothing. The header changes to
`● LIVE SERPAPI` and shows your remaining searches as `FUEL`; a key the
service refuses shows as `● KEY REJECTED` instead. The key is saved in the
settings file on your own computer and sent only to serpapi.com.

Searches are precious, so the app is careful with them:

- The launch button always shows what a scan will cost (`7 CALLS`): one
  search for each date it compares.
- A whole year of daily fares for the heatmap is 6 searches, not 365 (see
  Fare intel below).
- Every answer is kept for 6 hours. Asking the same question again is free.
- Launch a new scan before the old one finishes and the old one's
  remaining dates are dropped, not fetched.

SerpApi also limits how many searches an account may run in an hour (your
account page shows the figure). If either limit is reached the app says
`OUT OF FUEL` with the service's own reason.

To check the key and the service without opening the app:

```sh
icarus-search --probe SYD-PER
```

That runs one search and one price calendar for the route (2 searches) and
prints what came back, including whether the two agree on the day's fare.

## Settings

**F2** opens them: API key, currency, colour theme, symbols, continent
colours, borders, text size and how long results are kept. Arrow keys choose
and change, `Enter` opens an editor where there is one, `T` tests the key
again and `S` saves on the spot. Every change is saved as it is made in any
case, and the screen shows where the file is and when it was last written.

## Using it

Fill in the form on the left, press **S**.

| Key              | Does                                                              |
|------------------|-------------------------------------------------------------------|
| `S`              | Scan: search the route (and nearby dates if FLEX is above zero)   |
| `Tab`            | Move between panes                                                |
| arrows / `hjkl`  | Move within a pane; left/right changes a form value               |
| `Enter`          | Form: edit the field. Tracks: pick the return, or open the dossier |
| `Space`          | Full dossier for the selected flight                              |
| `O`              | Open the search in your browser to book                           |
| `1` to `7`       | Sort by price / best value / duration / departure / arrival / stops / carrier. Same key again reverses |
| `[` `]`          | Previous / next scanned date                                      |
| `I`              | Fare intel: when to fly, when to buy                              |
| `V`              | Survey: price every day of the coming months                      |
| `X`              | Swap origin and destination                                       |
| `G` `F` `C` `0`  | Globe or flat map / fullscreen / centre on route / reset view     |
| `>` `<`          | Finer / coarser map (smaller or larger text; Windows console)     |
| `B` `P` `M`      | Country borders / continent colours / drawing style               |
| `$` `T`          | Choose the currency / colour theme                                |
| `F2`             | Settings: API key, currency, theme and the rest                   |
| `Shift+K`        | Straight to the API key box (`Ctrl+V` or right-click pastes)      |
| `?`              | Key reference                                                     |
| `Q`              | Quit                                                              |

**Mouse:** click selects, double-click opens, the wheel scrolls. Click a
column heading in the flights table to sort by it, and again to reverse.
Click the currency or `F2 SETTINGS` in the top bar to open them. On the globe, click an
airport to set the destination, right-click to set the origin, drag to
rotate, and scroll to zoom toward the pointer.

The globe sits in the middle of its pane and stays there: choosing a route
or zooming turns it and resizes it, never moves it. To move it yourself,
hold `Shift` and drag (or `Ctrl` and drag: some terminals keep Shift+drag
for selecting text), or press `Shift` with the arrow keys while the globe
has the focus. It stays where you put it; `0` puts it back in the middle.

**Colours.** A trip runs from cyan (where you start) to pink (where you
are going), one step of colour per flight. The map, the form and the
itinerary all use the same colours, so a stop can be matched across them at
a glance. The trip type is a solid tag wherever a price appears, amber for
one-way and white for round-trip, because the same number means very
different things.

**Currency.** Australian dollars to begin with. Press `$` (or click the
currency) to pick another, or type any three-letter code. Changing it
re-prices the search rather than relabelling the old figures.

**Reading a track.** `0840  1848+1  25H08M  AMS 11H00  HND 1H10` leaves at
08:40, lands 18:48 the next day, takes 25 hours 8 minutes in all, and stops
in Amsterdam for 11 hours and Tokyo Haneda for 1 hour 10. The layover cell
turns red for a connection under an hour and amber for an overnight or
5-hour-plus wait. Tags: `LOW` cheapest, `FAST` quickest, `BEST` best value,
`ACE` cheapest and quickest at once.

**Times** are local to each airport, 24-hour, written `HHMM`. Time windows
are typed as `0600-1130`; `2200-0500` wraps past midnight.

**Dates.** `FLEX` widens the search around the departure date: `± 3 DAYS`
looks three days either side, and `OR UP TO 7 LATER` looks at that day and
the seven after it but never earlier. For a trip at short notice, put the
cursor on `DEPART` and press `Backspace` to jump to tomorrow, set `FLEX` to
`OR UP TO 7 LATER`, and scan: the price radar lists each day, and the
message at the bottom names both the cheapest day and the soonest.

**Round trips** are chosen in two steps, as on Google Flights: pick an
outbound flight (prices shown are for the whole trip), press Enter, then
pick the return.

## Fare intel: the two clocks

A fare depends on two different dates, and `I` shows both.

The screen explains itself: a short guide sits beside the grid when the
window is wide enough, and `?` opens the full one.

**When to fly.** A year laid out week by week: each square is a day you
could leave, columns are weeks and rows are Monday to Sunday. Green days are
cheaper than most of the year and red days dearer. The colour is a day's
rank, not its price, so one very dear Christmas week does not wash every
other day out to the same shade; the scale under the grid gives the actual
cheapest, middle and dearest fares. Each month's cheapest measured day is
marked `▼` and its dearest `▲`, and the table underneath names them. The
line under the grid says how the day under the cursor ranks ("cheaper than
93% of the year"). The date-picker calendar uses the same colours.

**When to buy.** For the day under the cursor, how that exact trip's price
has moved over the last couple of months. The verdict is written above the
chart in words ("below usual: a good price", "down 20% in 2 weeks"). The
usual price range is a shaded band across the chart, and the line is green
below it, amber inside it and red above it. The same band is drawn in grey
over the price-radar bars.

Searching each day of a year one at a time would be 365 searches, more than
a month's free allowance. A **survey** (`V`) asks for Google's price
calendar instead: one call returns the lowest fare for every departure date
in about two months, so the whole year is 6 calls and every day in it is a
fare that was seen, not an estimate. The app asks before spending them, and
skips any two months it priced within the last day. The simulator costs
nothing, so there the year is surveyed as soon as you open the screen.

Days the calendar has no fare for (no flights, or too far ahead to be on
sale) stay blank. A gap of a couple of weeks between known days is bridged
with an estimate, drawn fainter so the two are never mixed up.

Every fare the app ever sees is logged on your computer, so the map fills in
the more you use it.

One caution the screen also gives: looking at a single moment, "cheap
because it is far ahead" and "cheap because it is February" cannot be told
apart. The when-to-buy chart is the part that isolates timing.

## Things it cannot do

- **Book.** `O` hands the search to your browser.
- **Choose a seat.** Cabin class and legroom are shown; the actual seat is
  picked while booking.
- **Promise a price.** Fares move by the minute. Treat the number as a
  strong hint and confirm it at the booking step.
- **Search without SerpApi.** If their service or their free plan changes,
  live mode changes with it. The data source sits behind one small
  interface (`src/provider/mod.rs`) so another can be added.

## A sharper map

The map is drawn in braille: eight dots to every character, which is the
finest any terminal's text can go. So the detail you see depends on how many
characters fit in the window. **Smaller text in a bigger window is a finer
map.**

Both are yours to set; the app never resizes anything unasked.

- **`F`** gives the whole window to the globe, and `F` or `Esc` gives it
  back.
- **`>` and `<`** make the text smaller or larger, and with it the map
  finer or coarser. The window stays exactly the size and place it was;
  only the text changes, so smaller text means more of everything fits.
  Your choice is remembered for next time. This works in the classic
  Windows console, which is what a double-clicked program gets on
  Windows 10. Maximise the window (or press `Alt+Enter` for true full
  screen) to give it room.
- **Windows Terminal:** hold `Ctrl` and press `-` or `+`.
- **Mac:** hold `Cmd` and press `-` or `+`.

One thing the app does do for itself: the classic Windows console's usual
font has no braille, so the app borrows one that does (Cascadia Mono, if it
is installed) for as long as it runs, and puts the old one back on exit. If
there is no such font, the map is drawn as line art instead. Setting
`charset = "safe"` in the settings file turns the borrowing off.

## Troubleshooting

**The globe is a mess of boxes or question marks.** Your terminal's font
has no braille characters. Press `M` to switch drawing style, or start with
`--charset safe`. This is normally detected automatically.

**The map looks coarse.** See "A sharper map" above: make the text smaller.

**Colours look wrong.** Press `T` for the other theme. The app uses the
256-colour palette, which every current terminal supports.

**"API KEY REJECTED".** Press `F2`, then `Enter` on `API KEY`, and paste the
key again.

**Ctrl+V types nothing in the key box.** The clipboard holds no text (a
picture or a file, say). Copy the key again and paste; a right-click and
`Shift+Insert` paste too.

**"OUT OF FUEL".** The month's searches are used up, or more than the plan
allows in one hour; the message carries the service's own wording. Cached
results and the fare log still work; the monthly allowance resets monthly
and the hourly one within the hour.

**Is it the app or the service?** `icarus-search --probe SYD-PER` asks the
service two real questions and prints the answers. Add `--dump replies` to
keep exactly what it sent back, in a folder called `replies`.

## Files it keeps

**Settings: `icarus-search.toml`, in the same folder as the program.** It is
created the first time the app runs and holds the currency, theme, text
size, API key and your last search. Change it from inside the app with `F2`;
it is also plain text, safe to edit by hand while the app is closed. Move
the program and this file together and your settings
come along. If the program's folder cannot be written to, the file is kept
in your user settings folder instead.

Because the key lives in that file, do not hand someone your copy of it
unless you mean to hand them the key.

| What            | Windows                                | Mac                                            | Linux                           |
|-----------------|----------------------------------------|------------------------------------------------|---------------------------------|
| Fare log        | `%LOCALAPPDATA%\icarus-search\data\`   | `~/Library/Application Support/icarus-search/` | `~/.local/share/icarus-search/` |
| Cached searches | `%LOCALAPPDATA%\icarus-search\cache\`  | `~/Library/Caches/icarus-search/`              | `~/.cache/icarus-search/`       |

Deleting any of them is safe.

---

## Building from source

Needs [Rust](https://rustup.rs) 1.86 or newer. Nothing else: TLS is pure
Rust and the map data is compiled in.

```bash
cargo build --release
```

The binary is `target/release/icarus-search`.

### Baking a key into a build

To hand someone a copy that works with no setup, compile your key in. It is
read from an environment variable at build time, so it never appears in the
source or in git history.

```bash
ICARUS_BAKED_KEY=your-serpapi-key cargo build --release
```

PowerShell:

```powershell
$env:ICARUS_BAKED_KEY = "your-serpapi-key"; cargo build --release
```

Be clear-eyed about what this is: anyone who has the binary can extract the
key. That is a fair trade for a free-tier key shared with a friend, and a
bad one for a paid key. A key entered in the settings (`F2`), the
`ICARUS_API_KEY` environment variable, or `--key` all take precedence over
the baked one.

The release workflow (`.github/workflows/release.yml`) does the same from a
repository secret named `ICARUS_BAKED_KEY`, if one is set. Remember that
release binaries of a public repository are public.

### Releasing

Push a version tag and GitHub Actions builds all four binaries and attaches
them to a release:

```bash
git tag v0.1.0 && git push origin v0.1.0
```

### Testing

```bash
cargo test
```

The interface is tested without a terminal: a script drives the app and one
frame is rendered to text.

```bash
cargo run -- --snapshot 120x30 --keys "from:LHR to:NRT s wait"
```

Script tokens: `from:XXX` and `to:XXX` set airports; `wait` lets searches
finish; `pause` waits a second and a half;
`enter esc tab up down left right space bksp pgup pgdn f2` are keys;
`text:hello` types and `paste:hello` pastes; anything else is typed as
individual keys. Add `--bench` to time the renderer.

The live source is never touched by the tests. `--demo --probe SYD-PER`
runs the same check as `--probe` against the simulator, and the parsers are
tested against the sample replies in SerpApi's own documentation.

`--keys` also works without `--snapshot`: the app starts on the simulator
with the script already applied, and saves nothing. That is how a real
window is put into a known state to be looked at.

Text snapshots carry no colour and no font, and both have bitten this
project. On Windows, `tools/shoot.ps1` starts the app in a real classic
console window, photographs that one window to a PNG without taking the
keyboard, and closes it:

```powershell
tools\shoot.ps1 -Exe target\debug\icarus-search.exe -Out shot.png -Shell -Keys "from:SYD to:SGN s wait" -Extra "--demo --text-size 16" -Width 1920 -Height 1040
```

### Making "BEST" yours

The `BEST` sort ranks by price plus the cost of your time. How much a tight
connection or a night in a terminal should count against a flight is a
matter of taste, and is left as one small function to fill in:
`layover_pain` in [`src/score.rs`](src/score.rs).

### Layout

| Path                 | What                                                          |
|----------------------|---------------------------------------------------------------|
| `src/geo.rs`         | Sphere maths: projection, great circles, framing a route      |
| `src/ui/canvas.rs`   | Sub-cell dot canvas (braille, with line-art and block fallbacks) |
| `src/ui/globe.rs`    | The globe: land, coast, arcs, labels                          |
| `src/continents.rs`  | Which continent a coordinate is on, for the map's colours     |
| `src/console.rs`     | What the console can draw; font and text size on Windows      |
| `src/currency.rs`    | The currency list and how each is written                     |
| `src/config.rs`      | The settings file beside the program                          |
| `src/ui/intel.rs`    | Fare heatmap and booking curve                                |
| `src/app.rs`         | State and input handling                                      |
| `src/worker.rs`      | Background thread, so the interface never waits on the network |
| `src/provider/`      | Data sources: SerpApi (live) and the simulator                |
| `src/farelog.rs`     | The on-disk log of every fare seen                            |
| `data/`              | Embedded airports and map data, and the script that built them |

## Credits

- Airports: [OurAirports](https://ourairports.com/data/), public domain.
- Coastlines, borders, land: [Natural Earth](https://www.naturalearthdata.com/),
  public domain.
- Built on [ratatui](https://ratatui.rs).
- The stippled-land look was inspired by
  [tui-globe](https://github.com/d10n/tui-globe); no code is shared.

MIT licensed. See [LICENSE](LICENSE).
