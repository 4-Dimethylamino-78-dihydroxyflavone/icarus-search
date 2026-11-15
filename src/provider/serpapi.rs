//! Live fares from Google Flights via SerpApi (https://serpapi.com/google-flights-api).
//!
//! Two of its engines are used. `google_flights` is a search: the flights
//! on offer for one departure date. `google_flights_calendar` is the price
//! calendar: the lowest fare for every date in a range, in a single call.
//!
//! Round trips take two searches: the first returns outbound options, each
//! with a `departure_token`; passing that token back returns the matching
//! return flights. Prices on both lists are totals for the whole trip.

use std::{collections::BTreeMap, fs, path::PathBuf, time::Duration};

use chrono::{NaiveDate, NaiveDateTime};
use serde::{Deserialize, de::DeserializeOwned};

use super::{Provider, ProviderError, Quota};
use crate::{
    currency,
    model::{Cabin, Itinerary, Layover, PriceInsights, Query, SearchResult, Segment, Stage, Stops},
};

const SEARCH_URL: &str = "https://serpapi.com/search.json";
const ACCOUNT_URL: &str = "https://serpapi.com/account.json";

pub struct SerpApi {
    key: String,
    agent: ureq::Agent,
    /// Folder to keep every raw reply in (`--dump`), for seeing exactly
    /// what the service said when a result looks wrong.
    dump: Option<PathBuf>,
    kept: u32,
}

impl SerpApi {
    pub fn new(key: String, dump: Option<PathBuf>) -> Self {
        let config = ureq::Agent::config_builder()
            // A search that matches the website takes its time.
            .timeout_global(Some(Duration::from_secs(60)))
            // Error replies carry a JSON body explaining why; we want to read it.
            .http_status_as_error(false)
            .build();
        Self {
            key,
            agent: ureq::Agent::new_with_config(config),
            dump,
            kept: 0,
        }
    }

    fn get(&self, url: &str, params: &[(&str, String)]) -> Result<(u16, String), ProviderError> {
        let mut request = self.agent.get(url).query("api_key", &self.key);
        for (name, value) in params {
            request = request.query(*name, value);
        }
        let mut response = request
            .call()
            .map_err(|e| ProviderError::Network(scrub(&e.to_string(), &self.key)))?;
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|e| ProviderError::Network(scrub(&e.to_string(), &self.key)))?;
        Ok((status, body))
    }

    /// One call to a flights engine, kept on disk when that was asked for.
    /// (The account reply is never kept: it holds the key and an address.)
    fn flights(
        &mut self,
        label: &str,
        params: &[(&str, String)],
    ) -> Result<(u16, String), ProviderError> {
        let (status, body) = self.get(SEARCH_URL, params)?;
        if let Some(dir) = &self.dump {
            self.kept += 1;
            let _ = fs::create_dir_all(dir);
            let _ = fs::write(
                dir.join(format!("{:03}-{label}.json", self.kept)),
                scrub(&body, &self.key),
            );
        }
        Ok((status, body))
    }
}

/// The key travels in the query string, so make sure an error message that
/// echoes the URL can never put it on screen.
fn scrub(message: &str, key: &str) -> String {
    if key.is_empty() {
        message.to_string()
    } else {
        message.replace(key, "***")
    }
}

impl Provider for SerpApi {
    fn name(&self) -> &'static str {
        "SERPAPI"
    }

    fn is_live(&self) -> bool {
        true
    }

    fn search(&mut self, q: &Query, stage: &Stage) -> Result<SearchResult, ProviderError> {
        let (status, body) = self.flights("search", &search_params(q, stage))?;
        parse_response(status, &body, q)
    }

    fn calendar(
        &mut self,
        q: &Query,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<(NaiveDate, f64)>, ProviderError> {
        let (status, body) = self.flights("calendar", &calendar_params(q, from, to))?;
        parse_calendar(status, &body)
    }

    fn quota(&mut self) -> Result<Option<Quota>, ProviderError> {
        // The account endpoint is free and does not count against the quota.
        let (status, body) = self.get(ACCOUNT_URL, &[])?;
        parse_account(status, &body)
    }
}

/// What both engines are told about the trip. Never the API key, so these
/// can be tested and logged safely.
fn trip_params(q: &Query) -> Vec<(&'static str, String)> {
    let mut p = vec![
        ("hl", "en".to_string()),
        ("departure_id", q.origin.clone()),
        ("arrival_id", q.dest.clone()),
        ("type", if q.ret.is_some() { "1" } else { "2" }.to_string()),
        ("currency", q.currency.clone()),
        ("adults", q.adults.max(1).to_string()),
        (
            "travel_class",
            match q.cabin {
                Cabin::Economy => "1",
                Cabin::Premium => "2",
                Cabin::Business => "3",
                Cabin::First => "4",
            }
            .to_string(),
        ),
    ];
    // Shop as the country the currency belongs to, so the fares are the
    // ones its owner would be shown at home.
    if let Some(country) = currency::market(&q.currency) {
        p.push(("gl", country.to_string()));
    }
    let stops = match q.stops {
        Stops::Any => None,
        Stops::Nonstop => Some("1"),
        Stops::OneOrFewer => Some("2"),
        Stops::TwoOrFewer => Some("3"),
    };
    if let Some(code) = stops {
        p.push(("stops", code.to_string()));
    }
    if !q.airlines.is_empty() {
        p.push(("include_airlines", q.airlines.join(",")));
    }
    // The API filters by whole hours; minute-exact trimming happens locally.
    match (q.dep_window.hours(), q.arr_window.hours()) {
        (Some((a, b)), Some((c, d))) => p.push(("outbound_times", format!("{a},{b},{c},{d}"))),
        (Some((a, b)), None) => p.push(("outbound_times", format!("{a},{b}"))),
        (None, Some((c, d))) => p.push(("outbound_times", format!("0,23,{c},{d}"))),
        (None, None) => {}
    }
    p
}

fn search_params(q: &Query, stage: &Stage) -> Vec<(&'static str, String)> {
    let mut p = trip_params(q);
    p.push(("engine", "google_flights".to_string()));
    p.push(("outbound_date", q.depart.to_string()));
    if let Some(ret) = q.ret {
        p.push(("return_date", ret.to_string()));
    }
    // Include the fares Google tucks behind "view more flights".
    p.push(("show_hidden", "true".to_string()));
    // Left to itself the service answers fast with a list that can differ
    // from the website's. This asks for exactly what a browser would show
    // and waits the extra seconds for it: a fare that is not the one on
    // the booking page is worse than a slow one.
    p.push(("deep_search", "true".to_string()));
    if let Stage::Return { token } = stage {
        p.push(("departure_token", token.clone()));
    }
    p
}

fn calendar_params(q: &Query, from: NaiveDate, to: NaiveDate) -> Vec<(&'static str, String)> {
    let mut p = trip_params(q);
    p.push(("engine", "google_flights_calendar".to_string()));
    p.push(("date_range_start", from.to_string()));
    p.push(("date_range_end", to.to_string()));
    if let Some(ret) = q.ret {
        // Each departure date is priced with a return this many days on.
        p.push((
            "round_trip_days",
            (ret - q.depart).num_days().max(0).to_string(),
        ));
    }
    p
}

// Replies are parsed leniently: every field is optional, so a schema tweak
// upstream degrades one detail instead of breaking the whole search.

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawReply {
    error: Option<String>,
    search_metadata: Option<RawMeta>,
    best_flights: Vec<RawItinerary>,
    other_flights: Vec<RawItinerary>,
    price_insights: Option<RawInsights>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawMeta {
    google_flights_url: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawItinerary {
    flights: Vec<RawFlight>,
    layovers: Vec<RawLayover>,
    total_duration: Option<f64>,
    carbon_emissions: Option<RawCarbon>,
    price: Option<f64>,
    departure_token: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawFlight {
    departure_airport: RawAirport,
    arrival_airport: RawAirport,
    duration: Option<f64>,
    airplane: Option<String>,
    airline: Option<String>,
    travel_class: Option<String>,
    flight_number: Option<String>,
    legroom: Option<String>,
    extensions: Vec<String>,
    overnight: Option<bool>,
    often_delayed_by_over_30_min: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawAirport {
    name: Option<String>,
    id: Option<String>,
    time: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawLayover {
    duration: Option<f64>,
    name: Option<String>,
    id: Option<String>,
    overnight: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawCarbon {
    this_flight: Option<f64>,
    difference_percent: Option<f64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawInsights {
    lowest_price: Option<f64>,
    price_level: Option<String>,
    typical_price_range: Vec<f64>,
    /// `[[unix_time, price], ...]`
    price_history: Vec<Vec<f64>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawAccount {
    error: Option<String>,
    total_searches_left: Option<f64>,
    searches_per_month: Option<f64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawCalendar {
    error: Option<String>,
    prices: Vec<RawDay>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawDay {
    outbound_date: Option<String>,
    price: Option<f64>,
}

/// Read a reply body. One that is not JSON at all (a proxy's error page,
/// say) is judged by its HTTP status alone.
fn decode<T: DeserializeOwned>(status: u16, body: &str) -> Result<T, ProviderError> {
    serde_json::from_str(body).map_err(|e| match status {
        200 => ProviderError::Parse(e.to_string()),
        401 => ProviderError::Auth(format!("HTTP {status}")),
        429 => ProviderError::Quota(format!("HTTP {status}")),
        _ => ProviderError::Api(format!("HTTP {status}")),
    })
}

/// What a reply's `error` field amounts to. `Ok(true)` means "nothing
/// found", which SerpApi reports as an error and this program does not.
fn verdict(status: u16, error: Option<String>) -> Result<bool, ProviderError> {
    let Some(message) = error else {
        return if status == 200 {
            Ok(false)
        } else {
            Err(ProviderError::Api(format!("HTTP {status}")))
        };
    };
    let lower = message.to_lowercase();
    if lower.contains("hasn't returned any results") || lower.contains("no results") {
        Ok(true)
    } else if status == 401 || lower.contains("api key") {
        Err(ProviderError::Auth(message))
    } else if status == 429
        || lower.contains("run out of searches")
        || lower.contains("rate limit")
        || lower.contains("throughput")
    {
        Err(ProviderError::Quota(message))
    } else {
        Err(ProviderError::Api(message))
    }
}

fn parse_account(status: u16, body: &str) -> Result<Option<Quota>, ProviderError> {
    let account: RawAccount = decode(status, body)?;
    verdict(status, account.error)?;
    Ok(account.total_searches_left.map(|left| Quota {
        left: left.max(0.0) as u32,
        per_month: account.searches_per_month.unwrap_or(0.0).max(0.0) as u32,
    }))
}

/// `[(departure date, lowest fare)]`, earliest first.
fn parse_calendar(status: u16, body: &str) -> Result<Vec<(NaiveDate, f64)>, ProviderError> {
    let reply: RawCalendar = decode(status, body)?;
    if verdict(status, reply.error)? {
        return Ok(Vec::new());
    }
    // Keyed by date, so the list comes out in order and a date listed
    // twice keeps its cheaper fare. Entries without a readable date or a
    // real price are dropped rather than guessed at.
    let mut by_date: BTreeMap<NaiveDate, f64> = BTreeMap::new();
    for day in reply.prices {
        let date = day
            .outbound_date
            .as_deref()
            .and_then(|text| text.parse::<NaiveDate>().ok());
        if let (Some(date), Some(price)) = (date, day.price.filter(|p| *p > 0.0)) {
            by_date
                .entry(date)
                .and_modify(|known| *known = known.min(price))
                .or_insert(price);
        }
    }
    Ok(by_date.into_iter().collect())
}

fn parse_response(status: u16, body: &str, q: &Query) -> Result<SearchResult, ProviderError> {
    let reply: RawReply = decode(status, body)?;
    let empty = verdict(status, reply.error)?;
    let itineraries = if empty {
        Vec::new()
    } else {
        reply
            .best_flights
            .into_iter()
            .chain(reply.other_flights)
            .filter_map(convert_itinerary)
            .collect()
    };
    let insights = reply.price_insights.map(|raw| PriceInsights {
        lowest: raw.lowest_price,
        level: raw.price_level,
        typical: match raw.typical_price_range[..] {
            [lo, hi, ..] => Some((lo, hi)),
            _ => None,
        },
        history: raw
            .price_history
            .iter()
            .filter_map(|point| point.get(1).copied())
            .collect(),
    });
    Ok(SearchResult {
        itineraries,
        insights,
        browser_url: reply
            .search_metadata
            .and_then(|m| m.google_flights_url)
            .or_else(|| Some(q.browser_url())),
        cache_age_min: None,
    })
}

/// `None` drops the itinerary: one we can't time is one we can't show honestly.
fn convert_itinerary(raw: RawItinerary) -> Option<Itinerary> {
    let segments: Vec<Segment> = raw
        .flights
        .into_iter()
        .map(convert_flight)
        .collect::<Option<_>>()?;
    if segments.is_empty() {
        return None;
    }
    let mut layovers: Vec<Layover> = raw
        .layovers
        .into_iter()
        .map(|l| Layover {
            airport: l.id.unwrap_or_default(),
            name: l.name.unwrap_or_default(),
            duration_min: l.duration.unwrap_or(0.0).max(0.0) as u32,
            overnight: l.overnight.unwrap_or(false),
        })
        .collect();
    if layovers.len() != segments.len() - 1 {
        // Rebuild from the timetable. Both clocks are local to the same
        // airport, so plain subtraction is correct.
        layovers = segments
            .windows(2)
            .map(|pair| Layover {
                airport: pair[0].to.clone(),
                name: pair[0].to_name.clone(),
                duration_min: (pair[1].dep - pair[0].arr).num_minutes().max(0) as u32,
                overnight: pair[1].dep.date() != pair[0].arr.date(),
            })
            .collect();
    }
    let summed = segments.iter().map(|s| s.duration_min).sum::<u32>()
        + layovers.iter().map(|l| l.duration_min).sum::<u32>();
    Some(Itinerary {
        total_min: raw.total_duration.map_or(summed, |m| m.max(0.0) as u32),
        segments,
        layovers,
        price: raw.price.filter(|p| *p > 0.0),
        // Reported in grams.
        emissions_kg: raw
            .carbon_emissions
            .as_ref()
            .and_then(|c| c.this_flight)
            .map(|grams| (grams / 1000.0).round() as u32),
        emissions_vs_typical_pct: raw
            .carbon_emissions
            .as_ref()
            .and_then(|c| c.difference_percent)
            .map(|pct| pct.round() as i32),
        token: raw.departure_token,
    })
}

fn convert_flight(raw: RawFlight) -> Option<Segment> {
    let time =
        |t: &Option<String>| NaiveDateTime::parse_from_str(t.as_deref()?, "%Y-%m-%d %H:%M").ok();
    let dep = time(&raw.departure_airport.time)?;
    let arr = time(&raw.arrival_airport.time)?;
    let flight_no = raw.flight_number.unwrap_or_default();
    Some(Segment {
        from: raw.departure_airport.id?,
        to: raw.arrival_airport.id?,
        from_name: raw.departure_airport.name.unwrap_or_default(),
        to_name: raw.arrival_airport.name.unwrap_or_default(),
        dep,
        arr,
        duration_min: raw.duration.unwrap_or(0.0).max(0.0) as u32,
        airline_code: flight_no
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string(),
        airline: raw.airline.unwrap_or_else(|| "UNKNOWN CARRIER".into()),
        flight_no,
        aircraft: raw.airplane,
        cabin: raw.travel_class,
        legroom: raw.legroom,
        overnight: raw.overnight.unwrap_or(false),
        often_delayed: raw.often_delayed_by_over_30_min.unwrap_or(false),
        extras: raw.extensions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TimeWindow;

    fn query() -> Query {
        Query {
            origin: "PEK".into(),
            dest: "AUS".into(),
            depart: NaiveDate::from_ymd_opt(2026, 12, 10).unwrap(),
            ret: NaiveDate::from_ymd_opt(2026, 12, 20),
            adults: 2,
            cabin: Cabin::Business,
            stops: Stops::OneOrFewer,
            airlines: vec!["NH".into(), "UA".into()],
            dep_window: TimeWindow(Some((360, 719))),
            arr_window: TimeWindow::ANY,
            currency: "EUR".into(),
        }
    }

    /// Cut down from the sample reply in SerpApi's Google Flights
    /// documentation, field for field, with one unreadable itinerary added.
    const SAMPLE: &str = r#"{
      "search_metadata": {"id": "651a585815afff2d53eb9a5e", "status": "Success",
        "google_flights_url": "https://www.google.com/travel/flights?hl=en&tfs=abc", "total_time_taken": 0.71},
      "search_parameters": {"engine": "google_flights", "hl": "en", "departure_id": "PEK", "arrival_id": "AUS",
        "outbound_date": "2026-12-10", "return_date": "2026-12-20", "currency": "USD"},
      "best_flights": [{
        "flights": [
          {"departure_airport": {"name": "Beijing Capital International Airport", "id": "PEK", "time": "2026-12-10 15:10"},
           "arrival_airport": {"name": "Haneda Airport", "id": "HND", "time": "2026-12-10 19:35"},
           "duration": 205, "airplane": "Boeing 787", "airline": "ANA",
           "airline_logo": "https://www.gstatic.com/flights/airline_logos/70px/NH.png", "travel_class": "Economy",
           "flight_number": "NH 962", "legroom": "31 in",
           "extensions": ["Average legroom (31 in)", "Wi-Fi for a fee", "Carbon emissions estimate: 133 kg"]},
          {"departure_airport": {"name": "Haneda Airport", "id": "HND", "time": "2026-12-10 21:05"},
           "arrival_airport": {"name": "Austin-Bergstrom International Airport", "id": "AUS", "time": "2026-12-10 17:50"},
           "duration": 765, "airplane": "Boeing 787", "airline": "ANA", "travel_class": "Economy",
           "flight_number": "NH 114", "ticket_also_sold_by": ["United"], "legroom": "32 in",
           "extensions": ["Above average legroom (32 in)"], "overnight": true, "often_delayed_by_over_30_min": true}
        ],
        "layovers": [{"duration": 90, "name": "Haneda Airport", "id": "HND"}],
        "total_duration": 1060,
        "carbon_emissions": {"this_flight": 1106000, "typical_for_this_route": 949000, "difference_percent": 17},
        "price": 2512, "type": "Round trip",
        "airline_logo": "https://www.gstatic.com/flights/airline_logos/70px/multi.png",
        "extensions": ["Checked baggage for a fee", "Bag and fare conditions depend on the return flight"],
        "departure_token": "TOKEN-1"
      }],
      "other_flights": [
        {"flights": [{"departure_airport": {"id": "PEK", "time": "2026-12-10 08:00"},
                      "arrival_airport": {"id": "AUS", "time": "garbage"}}], "price": 1}
      ],
      "price_insights": {"lowest_price": 2512, "price_level": "high", "typical_price_range": [1500, 2100],
                         "price_history": [[1691013600, 2000], [1691100000, 2512]]}
    }"#;

    #[test]
    fn parses_the_documented_reply_shape() {
        let result = parse_response(200, SAMPLE, &query()).unwrap();
        assert_eq!(
            result.itineraries.len(),
            1,
            "the untimeable itinerary is dropped"
        );
        let it = &result.itineraries[0];
        assert_eq!((it.origin(), it.dest(), it.stops()), ("PEK", "AUS", 1));
        assert_eq!(it.total_min, 1060);
        assert_eq!(it.price, Some(2512.0));
        assert_eq!(it.token.as_deref(), Some("TOKEN-1"));
        assert_eq!(it.emissions_kg, Some(1106));
        assert_eq!(it.emissions_vs_typical_pct, Some(17));
        assert_eq!(it.layovers[0].airport, "HND");
        assert_eq!(it.layovers[0].duration_min, 90);
        assert_eq!(it.segments[0].airline_code, "NH");
        assert_eq!(it.segments[0].legroom.as_deref(), Some("31 in"));
        assert!(it.segments[1].often_delayed && it.segments[1].overnight);
        let insights = result.insights.unwrap();
        assert_eq!(insights.typical, Some((1500.0, 2100.0)));
        assert_eq!(insights.history, vec![2000.0, 2512.0]);
        assert!(result.browser_url.unwrap().contains("tfs=abc"));
    }

    #[test]
    fn missing_layovers_are_rebuilt_from_the_timetable() {
        let body = SAMPLE.replace(
            r#""layovers": [{"duration": 90, "name": "Haneda Airport", "id": "HND"}],"#,
            "",
        );
        let result = parse_response(200, &body, &query()).unwrap();
        let layover = &result.itineraries[0].layovers[0];
        assert_eq!(
            (layover.airport.as_str(), layover.duration_min),
            ("HND", 90)
        );
    }

    #[test]
    fn errors_are_classified() {
        let q = query();
        let no_flights =
            r#"{"error": "Google Flights hasn't returned any results for this query."}"#;
        assert!(
            parse_response(200, no_flights, &q)
                .unwrap()
                .itineraries
                .is_empty()
        );
        let bad_key = r#"{"error": "Invalid API key. Your API key should be here: https://serpapi.com/manage-api-key"}"#;
        assert!(matches!(
            parse_response(401, bad_key, &q),
            Err(ProviderError::Auth(_))
        ));
        let spent = r#"{"error": "Your account has run out of searches."}"#;
        assert!(matches!(
            parse_response(429, spent, &q),
            Err(ProviderError::Quota(_))
        ));
        assert!(matches!(
            parse_response(500, "<html>oops</html>", &q),
            Err(ProviderError::Api(_))
        ));
        assert!(matches!(
            parse_response(401, "<html>denied</html>", &q),
            Err(ProviderError::Auth(_))
        ));
        assert!(matches!(
            parse_response(200, "not json", &q),
            Err(ProviderError::Parse(_))
        ));
        // The calendar engine fails the same way, through the same judge.
        assert_eq!(parse_calendar(200, no_flights), Ok(vec![]));
        assert!(matches!(
            parse_calendar(401, bad_key),
            Err(ProviderError::Auth(_))
        ));
        assert!(matches!(
            parse_calendar(429, spent),
            Err(ProviderError::Quota(_))
        ));
    }

    #[test]
    fn search_carries_every_filter_but_never_the_key() {
        let params = search_params(
            &query(),
            &Stage::Return {
                token: "TOKEN-1".into(),
            },
        );
        let get = |name: &str| {
            params
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("engine"), Some("google_flights"));
        assert_eq!(get("type"), Some("1"));
        assert_eq!(get("outbound_date"), Some("2026-12-10"));
        assert_eq!(get("return_date"), Some("2026-12-20"));
        assert_eq!(get("travel_class"), Some("3"));
        assert_eq!(get("stops"), Some("2"));
        assert_eq!(get("adults"), Some("2"));
        assert_eq!(get("include_airlines"), Some("NH,UA"));
        assert_eq!(get("outbound_times"), Some("6,11"));
        assert_eq!(get("currency"), Some("EUR"));
        assert_eq!(get("departure_token"), Some("TOKEN-1"));
        assert_eq!(
            get("deep_search"),
            Some("true"),
            "the same list a browser shows"
        );
        assert_eq!(get("gl"), None, "the euro has no one home market");
        assert_eq!(get("api_key"), None);
    }

    #[test]
    fn fares_are_asked_for_as_the_currency_s_home_country() {
        let mut q = query();
        q.currency = "AUD".into();
        for params in [
            search_params(&q, &Stage::Outbound),
            calendar_params(&q, q.depart, q.depart),
        ] {
            let gl = params
                .iter()
                .find(|(k, _)| *k == "gl")
                .map(|(_, v)| v.as_str());
            assert_eq!(gl, Some("au"));
        }
    }

    #[test]
    fn a_calendar_call_asks_for_a_range_and_keeps_the_trip_length() {
        let (from, to) = (
            NaiveDate::from_ymd_opt(2027, 1, 1).unwrap(),
            NaiveDate::from_ymd_opt(2027, 3, 2).unwrap(),
        );
        let params = calendar_params(&query(), from, to);
        let get = |name: &str| {
            params
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("engine"), Some("google_flights_calendar"));
        assert_eq!(get("date_range_start"), Some("2027-01-01"));
        assert_eq!(get("date_range_end"), Some("2027-03-02"));
        assert_eq!(get("round_trip_days"), Some("10"));
        assert_eq!(get("type"), Some("1"));
        assert_eq!(get("stops"), Some("2"));
        assert_eq!(get("include_airlines"), Some("NH,UA"));
        // A single date's search parameters have no place in a range.
        assert_eq!(get("outbound_date"), None);
        assert_eq!(get("deep_search"), None);
        assert_eq!(get("api_key"), None);
        let mut one_way = query();
        one_way.ret = None;
        let params = calendar_params(&one_way, from, to);
        assert!(params.iter().all(|(k, _)| *k != "round_trip_days"));
        assert!(params.iter().any(|(k, v)| *k == "type" && v == "2"));
    }

    /// The two sample replies in SerpApi's Google Flights Calendar
    /// documentation: a round trip, then a one-way in another currency.
    const CALENDAR_ROUND_TRIP: &str = r#"{
      "search_metadata": {"id": "6968fb0cfc8a2ba837a46a4e", "status": "Success",
        "google_flights_calendar_url": "https://www.google.com/travel/flights?hl=en&gl=us", "total_time_taken": 1.9},
      "search_parameters": {"engine": "google_flights_calendar", "departure_id": "CDG", "arrival_id": "AUS"},
      "prices": [
        {"outbound_date": "2026-01-21", "return_date": "2026-01-28", "price": 1214},
        {"outbound_date": "2026-01-22", "return_date": "2026-01-29", "price": 1214, "low_price": true},
        {"outbound_date": "2026-01-20", "return_date": "2026-01-27", "price": 1302}
      ]
    }"#;

    const CALENDAR_ONE_WAY: &str = r#"{
      "search_metadata": {"id": "6968feddfc8a2ba837a46a50", "status": "Success", "total_time_taken": 1.55},
      "search_parameters": {"engine": "google_flights_calendar", "type": "2", "departure_id": "KHI",
        "arrival_id": "ISB", "currency": "PKR"},
      "prices": [
        {"outbound_date": "2026-01-15", "price": 59163},
        {"outbound_date": "2026-01-16", "price": 27009},
        {"outbound_date": "2026-01-17", "price": 25061},
        {"outbound_date": "2026-01-18", "price": 27009},
        {"outbound_date": "2026-01-19", "price": 24950}
      ]
    }"#;

    #[test]
    fn parses_the_documented_calendar_replies() {
        let day = |d: u32| NaiveDate::from_ymd_opt(2026, 1, d).unwrap();
        let round_trip = parse_calendar(200, CALENDAR_ROUND_TRIP).unwrap();
        assert_eq!(
            round_trip,
            vec![(day(20), 1302.0), (day(21), 1214.0), (day(22), 1214.0)],
            "in date order"
        );
        let one_way = parse_calendar(200, CALENDAR_ONE_WAY).unwrap();
        assert_eq!(one_way.len(), 5);
        assert_eq!(one_way[0], (day(15), 59163.0));
        assert_eq!(one_way[4], (day(19), 24950.0));
    }

    #[test]
    fn calendar_entries_that_cannot_be_trusted_are_dropped() {
        let body = r#"{"prices": [
            {"outbound_date": "2026-01-15", "price": 300},
            {"outbound_date": "2026-01-15", "price": 250},
            {"outbound_date": "soon", "price": 100},
            {"outbound_date": "2026-01-16", "price": 0},
            {"outbound_date": "2026-01-17"},
            {"price": 90}
        ]}"#;
        let day = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        assert_eq!(
            parse_calendar(200, body),
            Ok(vec![(day, 250.0)]),
            "one real date, at its cheaper fare"
        );
        assert_eq!(parse_calendar(200, "{}"), Ok(vec![]));
    }

    #[test]
    fn the_account_check_reads_fuel_and_reports_a_rejected_key() {
        let account = r#"{"account_id": "1", "api_key": "SECRET", "account_email": "someone@example.com",
            "plan_name": "Free Plan", "searches_per_month": 250, "plan_searches_left": 244,
            "total_searches_left": 244, "this_month_usage": 6, "account_rate_limit_per_hour": 50}"#;
        assert_eq!(
            parse_account(200, account),
            Ok(Some(Quota {
                left: 244,
                per_month: 250
            }))
        );
        let bad_key = r#"{"error": "Invalid API key. Your API key should be here: https://serpapi.com/manage-api-key"}"#;
        assert!(matches!(
            parse_account(401, bad_key),
            Err(ProviderError::Auth(_))
        ));
        assert_eq!(
            parse_account(200, "{}"),
            Ok(None),
            "an account with no figures is not an error"
        );
    }

    #[test]
    fn the_key_never_leaks_into_error_text() {
        assert_eq!(
            scrub("GET /search?api_key=SECRET failed", "SECRET"),
            "GET /search?api_key=*** failed"
        );
    }
}
