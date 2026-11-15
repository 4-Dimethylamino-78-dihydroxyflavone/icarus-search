//! Offline simulator: plausible but entirely fictional fares, so the terminal
//! is fully usable (and testable) with no API key. Routes follow real
//! geography; the carriers are invented so nothing here can be mistaken for
//! a real airline's schedule.
//!
//! Output is deterministic per (route, date, cabin): the same question always
//! gets the same answer, which keeps the price radar stable between runs.

use std::{thread, time::Duration as StdDuration};

use chrono::{Datelike, Duration, Local, NaiveDate, Weekday};

use super::{Provider, ProviderError};
use crate::{
    airports::{self, Airport},
    cache::fnv1a,
    currency,
    geo::distance_km,
    model::{Cabin, Itinerary, Layover, PriceInsights, Query, SearchResult, Segment, Stage},
};

const CARRIERS: [(&str, &str); 8] = [
    ("IK", "Icarus Air"),
    ("DD", "Daedalus Airways"),
    ("HE", "Helios Pacific"),
    ("MI", "Minos Atlantic"),
    ("AD", "Ariadne Express"),
    ("TL", "Talos Jet"),
    ("ZE", "Zephyr Lines"),
    ("BO", "Boreas Nordic"),
];

/// Real connecting hubs, so simulated layovers land somewhere believable.
const HUBS: [&str; 30] = [
    "LHR", "CDG", "FRA", "AMS", "MAD", "IST", "DXB", "DOH", "SIN", "HKG", "HND", "ICN", "BKK",
    "DEL", "JFK", "ORD", "ATL", "DFW", "LAX", "SFO", "YYZ", "MEX", "PTY", "BOG", "GRU", "JNB",
    "ADD", "CAI", "SYD", "AKL",
];

const AMENITIES: [&str; 5] = [
    "Wi-Fi for a fee",
    "Free Wi-Fi",
    "In-seat power & USB outlets",
    "On-demand video",
    "Stream media to your device",
];

pub struct Demo {
    /// Pause like a real network call would, so loading states are visible.
    latency: bool,
}

impl Demo {
    pub fn new(latency: bool) -> Self {
        Self { latency }
    }

    fn pause(&self, millis: u64) {
        if self.latency {
            thread::sleep(StdDuration::from_millis(millis));
        }
    }
}

impl Provider for Demo {
    fn name(&self) -> &'static str {
        "SIMULATOR"
    }

    fn is_live(&self) -> bool {
        false
    }

    fn search(&mut self, q: &Query, stage: &Stage) -> Result<SearchResult, ProviderError> {
        // Stable per question, so the same search always waits as long.
        self.pause(250 + fnv1a(&q.cache_key("pause", stage)) % 450);
        quote(q, stage)
    }

    fn calendar(
        &mut self,
        q: &Query,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<(NaiveDate, f64)>, ProviderError> {
        self.pause(180);
        let mut fares = Vec::new();
        let mut day = from;
        while day <= to {
            // The same question a scan of that day would ask, so the
            // calendar and the flight list can never disagree.
            let mut one = q.clone();
            one.ret = q.ret.map(|ret| ret + (day - q.depart));
            one.depart = day;
            let mut found = quote(&one, &Stage::Outbound)?;
            one.trim(&Stage::Outbound, &mut found);
            if let Some(price) = found.lowest_price() {
                fares.push((day, price));
            }
            day += Duration::days(1);
        }
        Ok(fares)
    }
}

/// The flights on offer for one question: no waiting, same answer every time.
fn quote(q: &Query, stage: &Stage) -> Result<SearchResult, ProviderError> {
    let db = airports::db();
    let lookup = |code: &str| {
        db.get(code)
            .ok_or_else(|| ProviderError::Api(format!("unknown airport {code}")))
    };
    let (home, away) = (lookup(&q.origin)?, lookup(&q.dest)?);
    // On the return stage the token carries the round-trip total already
    // quoted for the chosen outbound flight.
    let (from, to, date, quoted) = match stage {
        Stage::Outbound => (home, away, q.depart, None),
        Stage::Return { token } => (
            away,
            home,
            q.ret.unwrap_or(q.depart),
            token
                .strip_prefix("sim:")
                .and_then(|p| p.parse::<f64>().ok()),
        ),
    };
    let mut rng = Rng::new(fnv1a(&format!(
        "{}>{}@{date}#{:?}",
        from.iata, to.iata, q.cabin
    )));

    let km = distance_km(from.pos, to.pos);
    // The model thinks in US dollars; a rough rate turns that into
    // believable figures in whatever currency was asked for.
    let fare = (40.0 + 0.055 * km)
        * date_factor(date, from, to)
        * cabin_factor(q.cabin)
        * f64::from(q.adults.max(1))
        * if q.ret.is_some() { 1.8 } else { 1.0 }
        * currency::per_usd(&q.currency);

    let mut itineraries: Vec<Itinerary> = plans(from, to, km, &mut rng)
        .into_iter()
        .map(|path| {
            let premium = match path.len() - 2 {
                0 => rng.range(1.05, 1.35), // nonstop costs more
                1 => rng.range(0.78, 1.10),
                _ => rng.range(0.68, 0.95),
            };
            let mut it = fly(&path, date, q.cabin, &mut rng);
            let price = match quoted {
                Some(total) => (total * rng.range(1.0, 1.16)).round(),
                None => (fare * premium).round(),
            };
            it.price = Some(price);
            if matches!(stage, Stage::Outbound) && q.ret.is_some() {
                it.token = Some(format!("sim:{price}"));
            }
            it
        })
        .filter(|it| q.stops.max().is_none_or(|max| it.stops() <= max))
        .filter(|it| {
            q.airlines.is_empty()
                || it
                    .segments
                    .iter()
                    .all(|s| q.airlines.contains(&s.airline_code))
        })
        .collect();
    itineraries.sort_by(|a, b| {
        a.price
            .unwrap_or(f64::MAX)
            .total_cmp(&b.price.unwrap_or(f64::MAX))
    });
    itineraries.truncate(18);
    // The cheapest return must cost exactly what the outbound list promised.
    if let (Some(total), Some(first)) = (quoted, itineraries.first_mut()) {
        first.price = Some(total);
    }

    let insights = itineraries.first().and_then(|it| it.price).map(|lowest| {
        let typical = (fare * 0.9, fare * 1.25);
        let level = if lowest < typical.0 {
            "low"
        } else if lowest > typical.1 {
            "high"
        } else {
            "typical"
        };
        // A 60-day random walk that ends on today's lowest fare.
        let mut history = vec![lowest];
        for _ in 0..59 {
            let prev = history[history.len() - 1];
            history.push((prev * rng.range(0.94, 1.07)).clamp(fare * 0.6, fare * 1.7));
        }
        history.reverse();
        PriceInsights {
            lowest: Some(lowest),
            level: Some(level.into()),
            typical: Some((typical.0.round(), typical.1.round())),
            history,
        }
    });

    Ok(SearchResult {
        itineraries,
        insights,
        browser_url: Some(q.browser_url()),
        cache_age_min: None,
    })
}

/// SplitMix64: a few lines, good statistical quality, fully deterministic.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len())]
    }
}

fn cabin_factor(cabin: Cabin) -> f64 {
    match cabin {
        Cabin::Economy => 1.0,
        Cabin::Premium => 1.6,
        Cabin::Business => 3.2,
        Cabin::First => 5.5,
    }
}

/// Day-to-day fare movement: weekends and holidays cost more, last-minute
/// costs more, plus stable per-date noise so each day has its own price.
fn date_factor(date: NaiveDate, from: &Airport, to: &Airport) -> f64 {
    let weekday = match date.weekday() {
        Weekday::Fri | Weekday::Sun => 1.18,
        Weekday::Sat => 1.06,
        Weekday::Tue | Weekday::Wed => 0.90,
        _ => 1.0,
    };
    let season = match (date.month(), date.day()) {
        (12, 15..) | (1, ..=5) | (7, _) | (8, _) => 1.25,
        _ => 1.0,
    };
    let lead_days = (date - Local::now().date_naive()).num_days();
    let lead = match lead_days {
        ..7 => 1.5,
        7..21 => 1.2,
        21..120 => 1.0,
        _ => 1.05,
    };
    let noise = Rng::new(fnv1a(&format!("{}{}{date}", from.iata, to.iata))).range(0.85, 1.2);
    weekday * season * lead * noise
}

/// Candidate routings as airport sequences: nonstops, one-stops through the
/// hubs that add the least distance, and a couple of two-stops on long hauls.
fn plans(
    from: &'static Airport,
    to: &'static Airport,
    km: f64,
    rng: &mut Rng,
) -> Vec<Vec<&'static Airport>> {
    let mut out = Vec::new();
    if (250.0..=15_500.0).contains(&km) {
        for _ in 0..2 + rng.below(3) {
            out.push(vec![from, to]);
        }
    }
    let db = airports::db();
    let detour_limit = if km < 2_000.0 { 2.4 } else { 1.45 };
    let mut hubs: Vec<(&'static Airport, f64)> = HUBS
        .iter()
        .filter_map(|code| db.get(code))
        .filter(|h| h.iata != from.iata && h.iata != to.iata)
        .map(|h| {
            (
                h,
                (distance_km(from.pos, h.pos) + distance_km(h.pos, to.pos)) / km.max(1.0),
            )
        })
        .filter(|(_, detour)| *detour <= detour_limit)
        .collect();
    hubs.sort_by(|a, b| a.1.total_cmp(&b.1));
    hubs.truncate(5);
    for (hub, _) in &hubs {
        out.push(vec![from, hub, to]);
        if rng.unit() < 0.5 {
            out.push(vec![from, hub, to]);
        }
    }
    if km > 6_000.0 {
        let mut added = 0;
        'pairs: for (h1, _) in &hubs {
            for (h2, _) in &hubs {
                let (d1, d2) = (distance_km(from.pos, h1.pos), distance_km(from.pos, h2.pos));
                let total = d1 + distance_km(h1.pos, h2.pos) + distance_km(h2.pos, to.pos);
                if h1.iata != h2.iata && d1 < d2 && total / km <= 1.6 {
                    out.push(vec![from, h1, h2, to]);
                    added += 1;
                    if added == 2 {
                        break 'pairs;
                    }
                }
            }
        }
    }
    out
}

/// Turn an airport sequence into timed flights and layovers.
fn fly(path: &[&'static Airport], date: NaiveDate, cabin: Cabin, rng: &mut Rng) -> Itinerary {
    let lead = rng.pick(&CARRIERS);
    // Mostly daytime departures, with the occasional red-eye.
    let start_min = if rng.unit() < 0.12 {
        rng.below(72) * 5
    } else {
        360 + rng.below(210) * 5
    };
    let mut clock =
        date.and_hms_opt(0, 0, 0).expect("midnight exists") + Duration::minutes(start_min as i64);
    let mut segments = Vec::new();
    let mut layovers = Vec::new();
    let mut flown_km = 0.0;

    for (i, pair) in path.windows(2).enumerate() {
        let (a, b) = (pair[0], pair[1]);
        if i > 0 {
            let wait: u32 = if rng.unit() < 0.2 {
                360 + rng.below(100) as u32 * 5 // the long, cheap connection
            } else {
                50 + rng.below(50) as u32 * 5
            };
            let landed = clock;
            clock += Duration::minutes(i64::from(wait));
            layovers.push(Layover {
                airport: a.iata.into(),
                name: a.name.into(),
                duration_min: wait,
                overnight: clock.date() != landed.date(),
            });
        }
        let carrier = if i == 0 || rng.unit() < 0.8 {
            lead
        } else {
            rng.pick(&CARRIERS)
        };
        let km = distance_km(a.pos, b.pos);
        flown_km += km;
        let minutes = (35.0 + km / rng.range(780.0, 880.0) * 60.0).round() as u32;
        // No timezone database on board: solar time is close enough for a
        // simulation, and keeps arrival clocks roughly believable.
        let tz = |ap: &Airport| (ap.pos.lon / 15.0).round() as i64;
        let dep = clock;
        let arr = dep + Duration::minutes(i64::from(minutes)) + Duration::hours(tz(b) - tz(a));
        clock = arr;
        segments.push(Segment {
            from: a.iata.into(),
            to: b.iata.into(),
            from_name: a.name.into(),
            to_name: b.name.into(),
            dep,
            arr,
            duration_min: minutes,
            airline: carrier.1.into(),
            airline_code: carrier.0.into(),
            flight_no: format!("{} {}", carrier.0, 10 + rng.below(980)),
            aircraft: Some(
                if km < 1_500.0 {
                    rng.pick(&["Airbus A320neo", "Boeing 737 MAX 8", "Embraer E195-E2"])
                } else if km < 6_000.0 {
                    rng.pick(&["Airbus A321XLR", "Boeing 787-8", "Airbus A330-900"])
                } else {
                    rng.pick(&[
                        "Airbus A350-900",
                        "Boeing 787-9",
                        "Boeing 777-300ER",
                        "Airbus A380",
                    ])
                }
                .into(),
            ),
            cabin: Some(cabin.label().into()),
            legroom: Some(
                match cabin {
                    Cabin::Economy => rng.pick(&["29 in", "30 in", "31 in", "32 in"]),
                    Cabin::Premium => "38 in",
                    Cabin::Business => "Lie-flat seat",
                    Cabin::First => "Individual suite",
                }
                .into(),
            ),
            overnight: arr.date() > dep.date(),
            often_delayed: rng.unit() < 0.08,
            extras: AMENITIES
                .iter()
                .filter(|_| rng.unit() < 0.45)
                .map(|s| s.to_string())
                .collect(),
        });
    }

    let total_min = segments.iter().map(|s| s.duration_min).sum::<u32>()
        + layovers.iter().map(|l| l.duration_min).sum::<u32>();
    let direct_km = distance_km(path[0].pos, path[path.len() - 1].pos);
    let per_km = 0.085 * cabin_factor(cabin).sqrt();
    let emissions = flown_km * per_km * rng.range(0.92, 1.1);
    let typical = (direct_km * per_km * 1.08).max(1.0);
    Itinerary {
        segments,
        layovers,
        total_min,
        price: None,
        emissions_kg: Some(emissions.round() as u32),
        emissions_vs_typical_pct: Some(((emissions - typical) / typical * 100.0).round() as i32),
        token: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Stops, TimeWindow};

    fn query(origin: &str, dest: &str, round_trip: bool) -> Query {
        let depart = Local::now().date_naive() + Duration::days(40);
        Query {
            origin: origin.into(),
            dest: dest.into(),
            depart,
            ret: round_trip.then(|| depart + Duration::days(7)),
            adults: 1,
            cabin: Cabin::Economy,
            stops: Stops::Any,
            airlines: vec![],
            dep_window: TimeWindow::ANY,
            arr_window: TimeWindow::ANY,
            currency: "USD".into(),
        }
    }

    #[test]
    fn itineraries_are_internally_consistent() {
        let result = Demo::new(false)
            .search(&query("LHR", "NRT", false), &Stage::Outbound)
            .unwrap();
        assert!(result.itineraries.len() >= 5);
        for it in &result.itineraries {
            assert_eq!(it.origin(), "LHR");
            assert_eq!(it.dest(), "NRT");
            assert_eq!(it.layovers.len(), it.stops());
            assert!(it.price.is_some_and(|p| p > 100.0));
            // Each flight leaves from where the previous one landed.
            for pair in it.segments.windows(2) {
                assert_eq!(pair[0].to, pair[1].from);
                assert!(pair[1].dep > pair[0].arr);
            }
            let flying: u32 = it.segments.iter().map(|s| s.duration_min).sum();
            let waiting: u32 = it.layovers.iter().map(|l| l.duration_min).sum();
            assert_eq!(it.total_min, flying + waiting);
        }
        let prices: Vec<f64> = result
            .itineraries
            .iter()
            .filter_map(|it| it.price)
            .collect();
        assert!(
            prices.windows(2).all(|w| w[0] <= w[1]),
            "sorted cheapest first"
        );
    }

    #[test]
    fn same_question_same_answer() {
        let q = query("JFK", "CDG", false);
        let a = Demo::new(false).search(&q, &Stage::Outbound).unwrap();
        let b = Demo::new(false).search(&q, &Stage::Outbound).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn filters_are_honoured() {
        let mut q = query("LHR", "SYD", false);
        q.stops = Stops::OneOrFewer;
        let result = Demo::new(false).search(&q, &Stage::Outbound).unwrap();
        assert!(result.itineraries.iter().all(|it| it.stops() <= 1));
        q.stops = Stops::Any;
        q.airlines = vec!["IK".into()];
        let result = Demo::new(false).search(&q, &Stage::Outbound).unwrap();
        assert!(
            result
                .itineraries
                .iter()
                .flat_map(|it| &it.segments)
                .all(|s| s.airline_code == "IK")
        );
    }

    #[test]
    fn round_trip_return_leg_honours_the_quoted_total() {
        let q = query("LHR", "JFK", true);
        let mut demo = Demo::new(false);
        let out = demo.search(&q, &Stage::Outbound).unwrap();
        let chosen = &out.itineraries[0];
        let token = chosen
            .token
            .clone()
            .expect("round trips carry a return token");
        let back = demo.search(&q, &Stage::Return { token }).unwrap();
        assert!(!back.itineraries.is_empty());
        assert_eq!(back.itineraries[0].origin(), "JFK");
        assert_eq!(back.itineraries[0].dest(), "LHR");
        assert_eq!(back.itineraries[0].price, chosen.price);
        assert!(back.itineraries.iter().all(|it| it.price >= chosen.price));
    }

    #[test]
    fn unknown_airports_are_an_error_not_a_panic() {
        assert!(
            Demo::new(false)
                .search(&query("LHR", "QQQ", false), &Stage::Outbound)
                .is_err()
        );
        let q = query("LHR", "QQQ", false);
        assert!(Demo::new(false).calendar(&q, q.depart, q.depart).is_err());
    }

    #[test]
    fn the_calendar_quotes_what_a_search_of_each_day_would_find() {
        let q = query("SYD", "PER", true);
        let mut demo = Demo::new(false);
        let fares = demo
            .calendar(&q, q.depart, q.depart + Duration::days(60))
            .unwrap();
        assert_eq!(
            fares.len(),
            61,
            "every day of the range, both ends included"
        );
        assert!(
            fares.windows(2).all(|pair| pair[0].0 < pair[1].0),
            "in date order"
        );
        for (day, price) in [fares[0], fares[17], fares[60]] {
            // A round trip keeps its length as the departure date moves.
            let mut one = q.clone();
            one.ret = q.ret.map(|ret| ret + (day - q.depart));
            one.depart = day;
            assert_eq!(
                demo.search(&one, &Stage::Outbound).unwrap().lowest_price(),
                Some(price)
            );
        }
    }

    #[test]
    fn the_calendar_honours_the_same_filters_as_a_search() {
        let mut q = query("LHR", "SYD", false);
        let mut demo = Demo::new(false);
        let any = demo
            .calendar(&q, q.depart, q.depart + Duration::days(13))
            .unwrap();
        q.stops = Stops::Nonstop;
        let nonstop = demo
            .calendar(&q, q.depart, q.depart + Duration::days(13))
            .unwrap();
        // Nonstop over that distance is rarer and dearer, never cheaper.
        for (day, price) in &nonstop {
            let unfiltered = any.iter().find(|(d, _)| d == day).map(|(_, p)| *p).unwrap();
            assert!(
                *price >= unfiltered,
                "{day}: nonstop {price} under any-stops {unfiltered}"
            );
        }
    }
}
