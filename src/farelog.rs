//! Every fare ever sighted, kept on disk. A single search answers "what
//! does it cost today?"; this log is what lets the INTEL screen answer
//! "when is it cheap to fly?" and "is the price moving?".
//!
//! Stored as JSON lines so a write is one cheap append and a damaged line
//! costs one sighting, not the file.

use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
};

use chrono::{Datelike, Duration, Local, NaiveDate};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sighting {
    /// `Query::route_key`: the trip minus its date.
    pub route: String,
    pub depart: NaiveDate,
    /// Unix seconds when the fare was fetched.
    pub seen: i64,
    /// Lowest fare on offer for that departure date.
    pub price: f64,
}

pub struct FareLog {
    /// `None` keeps the log in memory only (simulator, tests).
    path: Option<PathBuf>,
    sightings: Vec<Sighting>,
}

impl FareLog {
    pub fn in_memory() -> Self {
        Self {
            path: None,
            sightings: Vec::new(),
        }
    }

    pub fn open(path: PathBuf) -> Self {
        let text = fs::read_to_string(&path).unwrap_or_default();
        let on_disk = text.lines().count();
        // Flights that left over a month ago can no longer inform a decision.
        let cutoff = Local::now().date_naive() - Duration::days(30);
        let sightings: Vec<Sighting> = text
            .lines()
            .filter_map(|line| serde_json::from_str::<Sighting>(line).ok())
            .filter(|s| s.depart >= cutoff)
            .collect();
        let log = Self {
            path: Some(path),
            sightings,
        };
        if log.sightings.len() < on_disk {
            log.rewrite();
        }
        log
    }

    pub fn record(&mut self, route: &str, depart: NaiveDate, price: f64, seen: i64) {
        self.record_all(route, &[(depart, price)], seen);
    }

    /// Log a batch of fares sighted together (a calendar's worth, say),
    /// opening the file once rather than once a fare. Returns how many
    /// were new.
    pub fn record_all(&mut self, route: &str, fares: &[(NaiveDate, f64)], seen: i64) -> usize {
        let mut lines = String::new();
        let mut added = 0;
        for &(depart, price) in fares {
            // Re-reading a cached result is the same sighting, not a new one.
            let duplicate = self
                .sightings
                .iter()
                .any(|s| s.depart == depart && s.route == route && (s.seen - seen).abs() < 3600);
            if duplicate || !price.is_finite() || price <= 0.0 {
                continue;
            }
            let sighting = Sighting {
                route: route.to_string(),
                depart,
                seen,
                price,
            };
            if let Ok(line) = serde_json::to_string(&sighting) {
                lines.push_str(&line);
                lines.push('\n');
            }
            self.sightings.push(sighting);
            added += 1;
        }
        if let (Some(path), false) = (&self.path, lines.is_empty()) {
            if let Some(dir) = path.parent() {
                let _ = fs::create_dir_all(dir);
            }
            if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
                let _ = file.write_all(lines.as_bytes());
            }
        }
        added
    }

    fn rewrite(&self) {
        let Some(path) = &self.path else { return };
        let text: String = self
            .sightings
            .iter()
            .filter_map(|s| serde_json::to_string(s).ok())
            .map(|line| line + "\n")
            .collect();
        let _ = fs::write(path, text);
    }

    /// The most recent sighting for each departure date on a route.
    pub fn latest(&self, route: &str) -> BTreeMap<NaiveDate, Sighting> {
        let mut map: BTreeMap<NaiveDate, Sighting> = BTreeMap::new();
        for s in self.sightings.iter().filter(|s| s.route == route) {
            match map.get(&s.depart) {
                Some(existing) if existing.seen >= s.seen => {}
                _ => {
                    map.insert(s.depart, s.clone());
                }
            }
        }
        map
    }

    /// Every sighting of one trip, oldest first: its price over booking time.
    pub fn history(&self, route: &str, depart: NaiveDate) -> Vec<(i64, f64)> {
        let mut points: Vec<(i64, f64)> = self
            .sightings
            .iter()
            .filter(|s| s.depart == depart && s.route == route)
            .map(|s| (s.seen, s.price))
            .collect();
        points.sort_by_key(|p| p.0);
        points
    }
}

/// Fares by departure date for one route, with gaps filled by estimate.
pub struct FareMap {
    pub known: BTreeMap<NaiveDate, Sighting>,
    pub lo: f64,
    pub hi: f64,
    /// Every measured fare, cheapest first, for ranking one against the rest.
    sorted: Vec<f64>,
}

impl FareMap {
    /// Sightings further apart than this are not bridged: two samples a
    /// month apart say nothing reliable about the days between them.
    const MAX_GAP_DAYS: i64 = 16;

    pub fn new(known: BTreeMap<NaiveDate, Sighting>) -> Self {
        let lo = known
            .values()
            .map(|s| s.price)
            .fold(f64::INFINITY, f64::min);
        let hi = known
            .values()
            .map(|s| s.price)
            .fold(f64::NEG_INFINITY, f64::max);
        let mut sorted: Vec<f64> = known.values().map(|s| s.price).collect();
        sorted.sort_by(f64::total_cmp);
        Self {
            known,
            lo,
            hi,
            sorted,
        }
    }

    /// The fare half the measured days are cheaper than.
    pub fn median(&self) -> Option<f64> {
        self.sorted.get(self.sorted.len() / 2).copied()
    }

    pub fn is_empty(&self) -> bool {
        self.known.is_empty()
    }

    /// `(fare, measured)`. `measured == false` is a straight-line estimate
    /// between the nearest sightings either side.
    pub fn at(&self, day: NaiveDate) -> Option<(f64, bool)> {
        if let Some(s) = self.known.get(&day) {
            return Some((s.price, true));
        }
        let (d0, before) = self.known.range(..day).next_back()?;
        let (d1, after) = self.known.range(day..).next()?;
        let gap = (*d1 - *d0).num_days();
        if gap > Self::MAX_GAP_DAYS {
            return None;
        }
        let t = (day - *d0).num_days() as f64 / gap as f64;
        Some((before.price + (after.price - before.price) * t, false))
    }

    /// Where a fare ranks among all those measured: 0.0 for the cheapest,
    /// 1.0 for the dearest, 0.5 for one that half the days undercut.
    ///
    /// Rank, not distance along the price scale. A year of fares has a long
    /// tail - a few very dear days at Christmas - and colouring by price
    /// would spend most of the palette on those few, leaving every ordinary
    /// day the same shade. By rank, the colours are shared out evenly and
    /// the cheap half of the year can be told from the dear half at a glance.
    pub fn heat(&self, price: f64) -> f64 {
        let n = self.sorted.len();
        if n < 2 || self.hi - self.lo < 1e-9 {
            return 0.0;
        }
        let below = self.sorted.partition_point(|p| *p < price);
        let not_above = self.sorted.partition_point(|p| *p <= price);
        // The middle of a run of equal fares, so ties share one colour.
        let position = (below + not_above.saturating_sub(1)) as f64 / 2.0;
        (position / (n - 1) as f64).clamp(0.0, 1.0)
    }

    /// Average fare per calendar month from `start` over `months` months:
    /// `(first day of month, average, measured days, covered days)`.
    pub fn by_month(
        &self,
        start: NaiveDate,
        months: u32,
    ) -> Vec<(NaiveDate, Option<f64>, u32, u32)> {
        let mut out = Vec::new();
        let mut first = start.with_day(1).unwrap_or(start);
        for _ in 0..months {
            let next = first
                .checked_add_months(chrono::Months::new(1))
                .unwrap_or(first);
            let (mut sum, mut covered, mut measured) = (0.0, 0u32, 0u32);
            let mut day = first.max(start);
            while day < next {
                if let Some((price, exact)) = self.at(day) {
                    sum += price;
                    covered += 1;
                    measured += u32::from(exact);
                }
                day += Duration::days(1);
            }
            out.push((
                first,
                (covered > 0).then(|| sum / f64::from(covered)),
                measured,
                covered,
            ));
            first = next;
        }
        out
    }

    /// The cheapest and the dearest day of the calendar month beginning at
    /// `first`, as `((day, fare), (day, fare))`, looking no earlier than
    /// `from`.
    ///
    /// Only measured days count. An estimate is a straight line between two
    /// measurements, so it can never be an extreme - and "the cheapest day
    /// this month" should be a fare somebody actually saw. `None` when the
    /// month has fewer than two measurements or they all agree: one sample
    /// has no best and no worst.
    pub fn month_extremes(
        &self,
        first: NaiveDate,
        from: NaiveDate,
    ) -> Option<((NaiveDate, f64), (NaiveDate, f64))> {
        let next = first.checked_add_months(chrono::Months::new(1))?;
        let mut days = self
            .known
            .range(first.max(from)..next)
            .map(|(day, s)| (*day, s.price));
        let start = days.next()?;
        let (low, high) = days.fold((start, start), |(low, high), day| {
            (
                if day.1 < low.1 { day } else { low },
                if day.1 > high.1 { day } else { high },
            )
        });
        (high.1 - low.1 > 1e-9).then_some((low, high))
    }

    /// Average fare by how far ahead the flight is, counted from `today`:
    /// `(label, average, days covered)`.
    pub fn by_lead(&self, today: NaiveDate) -> Vec<(&'static str, Option<f64>, u32)> {
        const BANDS: [(&str, i64, i64); 6] = [
            ("0-2 WKS", 0, 14),
            ("2-4 WKS", 15, 30),
            ("1-2 MO", 31, 60),
            ("2-3 MO", 61, 90),
            ("3-6 MO", 91, 180),
            ("6-12 MO", 181, 365),
        ];
        BANDS
            .iter()
            .map(|(label, from, to)| {
                let (mut sum, mut n) = (0.0, 0u32);
                for offset in *from..=*to {
                    if let Some((price, _)) = self.at(today + Duration::days(offset)) {
                        sum += price;
                        n += 1;
                    }
                }
                (*label, (n > 0).then(|| sum / f64::from(n)), n)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2027, m, d).unwrap()
    }

    #[test]
    fn latest_sighting_wins_and_history_is_ordered() {
        let mut log = FareLog::in_memory();
        log.record("R", day(3, 10), 500.0, 10_000);
        log.record("R", day(3, 10), 450.0, 90_000);
        log.record("R", day(3, 10), 450.0, 90_100); // within the hour: a re-read, ignored
        log.record("OTHER", day(3, 10), 99.0, 95_000);
        assert_eq!(log.latest("R")[&day(3, 10)].price, 450.0);
        assert_eq!(
            log.history("R", day(3, 10)),
            vec![(10_000, 500.0), (90_000, 450.0)]
        );
    }

    #[test]
    fn gaps_are_interpolated_but_only_when_narrow() {
        let mut log = FareLog::in_memory();
        log.record("R", day(3, 1), 400.0, 1);
        log.record("R", day(3, 9), 480.0, 2);
        log.record("R", day(5, 1), 900.0, 3);
        let map = FareMap::new(log.latest("R"));
        assert_eq!(map.at(day(3, 1)), Some((400.0, true)));
        assert_eq!(map.at(day(3, 5)), Some((440.0, false)));
        assert_eq!(map.at(day(4, 1)), None, "a 53-day gap is not bridged");
        assert_eq!(
            map.at(day(2, 20)),
            None,
            "no extrapolation before the first sighting"
        );
        assert_eq!((map.lo, map.hi), (400.0, 900.0));
        assert_eq!(map.heat(400.0), 0.0);
        assert_eq!(map.heat(900.0), 1.0);
    }

    #[test]
    fn heat_is_rank_so_one_dear_day_does_not_flatten_the_rest() {
        let mut log = FareLog::in_memory();
        // Four ordinary days and one Christmas.
        for (d, price) in [(1, 500.0), (2, 510.0), (3, 520.0), (4, 530.0), (5, 2000.0)] {
            log.record("R", day(3, d), price, 1);
        }
        let map = FareMap::new(log.latest("R"));
        // By price, 530 would sit 2% of the way up the scale: the same
        // colour as 500. By rank it is three quarters of the way up.
        assert_eq!(map.heat(500.0), 0.0);
        assert_eq!(map.heat(510.0), 0.25);
        assert_eq!(map.heat(530.0), 0.75);
        assert_eq!(map.heat(2000.0), 1.0);
        // A fare between two measured ones ranks between them.
        assert!(map.heat(515.0) > 0.25 && map.heat(515.0) < 0.5);
        assert_eq!(map.median(), Some(520.0));
        assert_eq!(FareMap::new(BTreeMap::new()).median(), None);
    }

    #[test]
    fn monthly_averages_count_measured_and_estimated_days() {
        let mut log = FareLog::in_memory();
        log.record("R", day(3, 1), 400.0, 1);
        log.record("R", day(3, 3), 600.0, 2);
        let map = FareMap::new(log.latest("R"));
        let months = map.by_month(day(3, 1), 2);
        assert_eq!(months[0], (day(3, 1), Some(500.0), 2, 3));
        assert_eq!(months[1], (day(4, 1), None, 0, 0));
    }

    #[test]
    fn month_extremes_are_measured_days_only() {
        let mut log = FareLog::in_memory();
        for (d, price) in [(2, 500.0), (9, 320.0), (16, 780.0), (23, 410.0)] {
            log.record("R", day(3, d), price, 1);
        }
        log.record("R", day(4, 6), 600.0, 1);
        let map = FareMap::new(log.latest("R"));
        let march = map.month_extremes(day(3, 1), day(3, 1));
        assert_eq!(march, Some(((day(3, 9), 320.0), (day(3, 16), 780.0))));
        // Looking only from the 10th, the 9th's bargain has already gone.
        let late = map.month_extremes(day(3, 1), day(3, 10));
        assert_eq!(late, Some(((day(3, 23), 410.0), (day(3, 16), 780.0))));
        // One sample has neither a best nor a worst; nor has an empty month.
        assert_eq!(map.month_extremes(day(4, 1), day(3, 1)), None);
        assert_eq!(map.month_extremes(day(5, 1), day(3, 1)), None);
    }

    #[test]
    fn a_batch_is_logged_once_and_survives_a_restart() {
        let path =
            std::env::temp_dir().join(format!("icarus-fares-batch-{}.jsonl", std::process::id()));
        let _ = fs::remove_file(&path);
        let soon = Local::now().date_naive() + Duration::days(20);
        let fares: Vec<(NaiveDate, f64)> = (0..5)
            .map(|i| (soon + Duration::days(i), 300.0 + i as f64))
            .collect();
        {
            let mut log = FareLog::open(path.clone());
            assert_eq!(log.record_all("R", &fares, 5_000), 5);
            // The same calendar read again within the hour adds nothing,
            // and a worthless price is never logged.
            assert_eq!(log.record_all("R", &fares, 5_100), 0);
            assert_eq!(
                log.record_all("R", &[(soon, 0.0), (soon, f64::NAN)], 90_000),
                0
            );
        }
        let reopened = FareLog::open(path.clone());
        assert_eq!(reopened.latest("R").len(), 5);
        assert_eq!(
            reopened.latest("R")[&(soon + Duration::days(4))].price,
            304.0
        );
        assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 5);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn log_survives_a_restart() {
        let path =
            std::env::temp_dir().join(format!("icarus-fares-test-{}.jsonl", std::process::id()));
        let _ = fs::remove_file(&path);
        let soon = Local::now().date_naive() + Duration::days(20);
        {
            let mut log = FareLog::open(path.clone());
            log.record("R", soon, 321.0, 1_000);
        }
        let reopened = FareLog::open(path.clone());
        assert_eq!(reopened.latest("R")[&soon].price, 321.0);
        let _ = fs::remove_file(path);
    }
}
