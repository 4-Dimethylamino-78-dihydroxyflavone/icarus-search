//! Provider-agnostic flight model. Every data source converts into these
//! types, so the UI never knows (or cares) where a fare came from.

use chrono::{NaiveDate, NaiveDateTime, NaiveTime, Timelike};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Cabin {
    #[default]
    Economy,
    Premium,
    Business,
    First,
}

impl Cabin {
    pub const ALL: [Cabin; 4] = [
        Cabin::Economy,
        Cabin::Premium,
        Cabin::Business,
        Cabin::First,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Cabin::Economy => "ECONOMY",
            Cabin::Premium => "PREMIUM ECONOMY",
            Cabin::Business => "BUSINESS",
            Cabin::First => "FIRST",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Stops {
    #[default]
    Any,
    Nonstop,
    OneOrFewer,
    TwoOrFewer,
}

impl Stops {
    pub const ALL: [Stops; 4] = [
        Stops::Any,
        Stops::Nonstop,
        Stops::OneOrFewer,
        Stops::TwoOrFewer,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Stops::Any => "ANY",
            Stops::Nonstop => "NONSTOP ONLY",
            Stops::OneOrFewer => "1 STOP MAX",
            Stops::TwoOrFewer => "2 STOPS MAX",
        }
    }

    pub fn max(self) -> Option<usize> {
        match self {
            Stops::Any => None,
            Stops::Nonstop => Some(0),
            Stops::OneOrFewer => Some(1),
            Stops::TwoOrFewer => Some(2),
        }
    }
}

/// A time-of-day window in minutes since midnight, inclusive at both ends.
/// `start > end` wraps past midnight (2200-0500). `None` means any time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TimeWindow(pub Option<(u16, u16)>);

impl TimeWindow {
    pub const ANY: Self = Self(None);

    pub const PRESETS: [(TimeWindow, &'static str); 5] = [
        (TimeWindow::ANY, "ANY TIME"),
        (TimeWindow(Some((0, 359))), "RED-EYE"),
        (TimeWindow(Some((360, 719))), "MORNING"),
        (TimeWindow(Some((720, 1079))), "AFTERNOON"),
        (TimeWindow(Some((1080, 1439))), "EVENING"),
    ];

    pub fn contains(self, t: NaiveTime) -> bool {
        let Some((start, end)) = self.0 else {
            return true;
        };
        let m = (t.hour() * 60 + t.minute()) as u16;
        if start <= end {
            (start..=end).contains(&m)
        } else {
            m >= start || m <= end
        }
    }

    /// "0600-1159", or "ANY".
    pub fn label(self) -> String {
        match self.0 {
            None => "ANY".into(),
            Some((s, e)) => format!("{:02}{:02}-{:02}{:02}", s / 60, s % 60, e / 60, e % 60),
        }
    }

    pub fn preset_name(self) -> Option<&'static str> {
        Self::PRESETS
            .iter()
            .find(|(w, _)| *w == self)
            .map(|(_, name)| *name)
    }

    /// Parse "HHMM-HHMM" (colons and spaces tolerated). Empty or "ANY" clears.
    pub fn parse(text: &str) -> Result<Self, String> {
        let cleaned: String = text
            .chars()
            .filter(|c| !c.is_whitespace() && *c != ':')
            .collect();
        if cleaned.is_empty() || cleaned.eq_ignore_ascii_case("any") {
            return Ok(Self::ANY);
        }
        let (a, b) = cleaned
            .split_once('-')
            .ok_or("USE HHMM-HHMM, E.G. 0600-1130")?;
        let minute = |s: &str| -> Result<u16, String> {
            if !(3..=4).contains(&s.len()) || !s.bytes().all(|c| c.is_ascii_digit()) {
                return Err(format!("'{s}' IS NOT A 24H TIME LIKE 0730"));
            }
            let n: u16 = s.parse().map_err(|_| "BAD TIME".to_string())?;
            let (h, m) = (n / 100, n % 100);
            match (h, m) {
                (24, 0) => Ok(1439),
                (0..=23, 0..=59) => Ok(h * 60 + m),
                _ => Err(format!("'{s}' IS OUT OF RANGE (0000-2359)")),
            }
        };
        Ok(Self(Some((minute(a)?, minute(b)?))))
    }

    /// Whole-hour bounds for APIs that filter by the hour. `None` when the
    /// window is open or wraps midnight (those are filtered locally instead).
    pub fn hours(self) -> Option<(u16, u16)> {
        let (s, e) = self.0?;
        (s <= e).then_some((s / 60, e / 60))
    }
}

/// Everything a provider needs to price one departure date.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Query {
    pub origin: String,
    pub dest: String,
    pub depart: NaiveDate,
    /// `Some` makes it a round trip.
    pub ret: Option<NaiveDate>,
    pub adults: u8,
    pub cabin: Cabin,
    pub stops: Stops,
    /// Include-only filter: IATA airline codes or alliance names. Empty = any.
    pub airlines: Vec<String>,
    /// Applied to the outbound leg.
    pub dep_window: TimeWindow,
    pub arr_window: TimeWindow,
    pub currency: String,
}

/// A round trip is searched in two steps: outbound options first, then the
/// return options that pair with one chosen outbound.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stage {
    Outbound,
    Return { token: String },
}

impl Query {
    /// Stable identity for the on-disk cache. Never includes the API key.
    pub fn cache_key(&self, provider: &str, stage: &Stage) -> String {
        let token = match stage {
            Stage::Outbound => "",
            Stage::Return { token } => token,
        };
        format!(
            "{provider}|{}|{}|{}|{:?}|{}|{:?}|{:?}|{}|{}|{}|{}|{token}",
            self.origin,
            self.dest,
            self.depart,
            self.ret,
            self.adults,
            self.cabin,
            self.stops,
            self.airlines.join("+"),
            self.dep_window.label(),
            self.arr_window.label(),
            self.currency,
        )
    }

    /// The trip minus its departure date: what the fare log files prices
    /// under, so fares for different dates of the same trip line up. Includes
    /// the provider, so simulated prices can never mix with live ones.
    pub fn route_key(&self, provider: &str) -> String {
        let trip = match self.ret {
            Some(ret) => format!("RT{}", (ret - self.depart).num_days()),
            None => "OW".to_string(),
        };
        format!(
            "{provider}|{}>{}|{trip}|{}|{:?}|{:?}|{}|{}|{}|{}",
            self.origin,
            self.dest,
            self.adults,
            self.cabin,
            self.stops,
            self.airlines.join("+"),
            self.dep_window.label(),
            self.arr_window.label(),
            self.currency,
        )
    }

    /// Minute-exact time-window filter. Providers can only filter by whole
    /// hours (or not at all), so the precise cut happens here.
    pub fn trim(&self, stage: &Stage, result: &mut SearchResult) {
        if matches!(stage, Stage::Outbound) {
            result.itineraries.retain(|it| {
                self.dep_window.contains(it.dep().time())
                    && self.arr_window.contains(it.arr().time())
            });
        }
    }

    /// A Google Flights search for this trip, for finishing the booking in
    /// a browser.
    pub fn browser_url(&self) -> String {
        let mut text = format!(
            "Flights from {} to {} on {}",
            self.origin, self.dest, self.depart
        );
        match self.ret {
            Some(ret) => text.push_str(&format!(" through {ret}")),
            None => text.push_str(" one way"),
        }
        format!(
            "https://www.google.com/travel/flights?q={}",
            text.replace(' ', "%20")
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub from: String,
    pub to: String,
    pub from_name: String,
    pub to_name: String,
    /// Local time at the departure airport.
    pub dep: NaiveDateTime,
    /// Local time at the arrival airport.
    pub arr: NaiveDateTime,
    pub duration_min: u32,
    pub airline: String,
    pub airline_code: String,
    pub flight_no: String,
    pub aircraft: Option<String>,
    pub cabin: Option<String>,
    pub legroom: Option<String>,
    pub overnight: bool,
    pub often_delayed: bool,
    pub extras: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layover {
    pub airport: String,
    pub name: String,
    pub duration_min: u32,
    pub overnight: bool,
}

/// Something about a layover worth a warning label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoverFlag {
    /// Under an hour: a delay on the first flight can lose the second.
    Tight,
    Overnight,
    /// Over five hours.
    Long,
}

impl Layover {
    pub fn flag(&self) -> Option<LayoverFlag> {
        if self.duration_min < 60 {
            Some(LayoverFlag::Tight)
        } else if self.overnight {
            Some(LayoverFlag::Overnight)
        } else if self.duration_min > 300 {
            Some(LayoverFlag::Long)
        } else {
            None
        }
    }
}

/// One bookable way of getting there: one or more flights joined by layovers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Itinerary {
    /// Never empty: providers drop itineraries they cannot fully parse.
    pub segments: Vec<Segment>,
    pub layovers: Vec<Layover>,
    pub total_min: u32,
    /// Total for all passengers. For round trips this is the whole trip.
    pub price: Option<f64>,
    pub emissions_kg: Option<u32>,
    pub emissions_vs_typical_pct: Option<i32>,
    /// Opaque handle for fetching the matching return flights.
    pub token: Option<String>,
}

impl Itinerary {
    pub fn dep(&self) -> NaiveDateTime {
        self.segments[0].dep
    }

    pub fn arr(&self) -> NaiveDateTime {
        self.segments[self.segments.len() - 1].arr
    }

    pub fn origin(&self) -> &str {
        &self.segments[0].from
    }

    pub fn dest(&self) -> &str {
        &self.segments[self.segments.len() - 1].to
    }

    pub fn stops(&self) -> usize {
        self.segments.len() - 1
    }

    /// Calendar days between departure and arrival ("+1").
    pub fn day_shift(&self) -> i64 {
        (self.arr().date() - self.dep().date()).num_days()
    }

    /// Distinct airline names in flying order.
    pub fn carriers(&self) -> Vec<&str> {
        let mut seen: Vec<&str> = Vec::new();
        for s in &self.segments {
            if !seen.contains(&s.airline.as_str()) {
                seen.push(&s.airline);
            }
        }
        seen
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PriceInsights {
    pub lowest: Option<f64>,
    /// "low" | "typical" | "high"
    pub level: Option<String>,
    pub typical: Option<(f64, f64)>,
    /// Daily lowest fare, oldest first.
    pub history: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchResult {
    pub itineraries: Vec<Itinerary>,
    pub insights: Option<PriceInsights>,
    pub browser_url: Option<String>,
    /// Minutes since this was fetched, when served from the disk cache.
    #[serde(default)]
    pub cache_age_min: Option<u32>,
}

impl SearchResult {
    pub fn lowest_price(&self) -> Option<f64> {
        self.itineraries
            .iter()
            .filter_map(|it| it.price)
            .min_by(f64::total_cmp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    #[test]
    fn windows_are_inclusive_and_can_wrap_midnight() {
        let morning = TimeWindow(Some((360, 719)));
        assert!(morning.contains(at(6, 0)) && morning.contains(at(11, 59)));
        assert!(!morning.contains(at(5, 59)) && !morning.contains(at(12, 0)));
        let night = TimeWindow(Some((1320, 300)));
        assert!(night.contains(at(23, 30)) && night.contains(at(2, 0)));
        assert!(!night.contains(at(12, 0)));
        assert!(TimeWindow::ANY.contains(at(3, 33)));
    }

    #[test]
    fn window_parsing() {
        assert_eq!(
            TimeWindow::parse("0600-1159"),
            Ok(TimeWindow(Some((360, 719))))
        );
        assert_eq!(
            TimeWindow::parse(" 06:00 - 11:59 "),
            Ok(TimeWindow(Some((360, 719))))
        );
        assert_eq!(TimeWindow::parse("any"), Ok(TimeWindow::ANY));
        assert_eq!(TimeWindow::parse(""), Ok(TimeWindow::ANY));
        assert_eq!(
            TimeWindow::parse("1800-2400"),
            Ok(TimeWindow(Some((1080, 1439))))
        );
        assert!(TimeWindow::parse("2560-0100").is_err());
        assert!(TimeWindow::parse("0600").is_err());
        assert_eq!(TimeWindow(Some((360, 719))).label(), "0600-1159");
        assert_eq!(TimeWindow(Some((360, 719))).preset_name(), Some("MORNING"));
    }

    #[test]
    fn api_hours_skip_wrapping_windows() {
        assert_eq!(TimeWindow(Some((360, 719))).hours(), Some((6, 11)));
        assert_eq!(TimeWindow(Some((1320, 300))).hours(), None);
        assert_eq!(TimeWindow::ANY.hours(), None);
    }

    #[test]
    fn browser_url_has_no_spaces() {
        let q = Query {
            origin: "LHR".into(),
            dest: "NRT".into(),
            depart: NaiveDate::from_ymd_opt(2026, 12, 10).unwrap(),
            ret: NaiveDate::from_ymd_opt(2026, 12, 20),
            adults: 1,
            cabin: Cabin::Economy,
            stops: Stops::Any,
            airlines: vec![],
            dep_window: TimeWindow::ANY,
            arr_window: TimeWindow::ANY,
            currency: "USD".into(),
        };
        let url = q.browser_url();
        assert!(!url.contains(' '));
        assert!(
            url.ends_with(
                "Flights%20from%20LHR%20to%20NRT%20on%202026-12-10%20through%202026-12-20"
            )
        );
    }
}
