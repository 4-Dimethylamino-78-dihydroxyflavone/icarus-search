//! The "BEST" ranking: one number per itinerary, lower is better.
//!
//! The idea is *effective cost*: the ticket price plus what the journey
//! costs you in time and aggravation, all expressed in fare currency. A
//! $40 saving is not a saving if it buys a nine-hour layover.
//!
//! This file is deliberately small and self-contained: it is the one place
//! where taste, not engineering, decides the answer. Tune it freely.

use crate::model::{Itinerary, Layover};

/// What an hour of door-to-door travel is worth, in US dollars.
/// Raise it and BEST favours fast trips; drop it to 0 and BEST equals PRICE.
pub const HOUR_VALUE: f64 = 12.0;

/// `per_usd` is the fare currency's rough rate (`currency::per_usd`): the
/// taste knobs here are written in dollars and scaled to match the fares,
/// so an hour is worth the same whether prices are in dollars or in yen.
pub fn deal_score(it: &Itinerary, per_usd: f64) -> f64 {
    // An unpriced itinerary can't be a deal; park it at the bottom.
    let price = it.price.unwrap_or(1.0e12);
    let hours = it.total_min as f64 / 60.0;
    price + per_usd * (HOUR_VALUE * hours + it.layovers.iter().map(layover_pain).sum::<f64>())
}

/// Extra cost, in US dollars, of enduring one layover *beyond* the time it
/// takes (time is already charged through `HOUR_VALUE`).
///
/// TODO(you): this is the taste knob. It currently returns 0, so BEST only
/// weighs price against total travel time. Things you might charge for:
///
/// - `l.duration_min < 60`: a tight connection risks a missed flight. A big
///   flat penalty (say 150) effectively bans them; a small one just nudges.
/// - `l.overnight`: sleeping in a terminal, or paying for a hotel.
/// - `l.duration_min > 300`: a dead afternoon in an airport. Or negative
///   pain, if a long stop is a free mini-visit to another city.
///
/// Whatever you return is added straight onto the price, so think of it as
/// "I'd pay this much to avoid it".
pub fn layover_pain(l: &Layover) -> f64 {
    let _ = l;
    0.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Segment;
    use chrono::NaiveDate;

    fn itinerary(price: f64, total_min: u32, layovers: Vec<Layover>) -> Itinerary {
        let t = NaiveDate::from_ymd_opt(2026, 12, 10)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap();
        Itinerary {
            segments: vec![Segment {
                from: "AAA".into(),
                to: "BBB".into(),
                from_name: String::new(),
                to_name: String::new(),
                dep: t,
                arr: t,
                duration_min: total_min,
                airline: "X".into(),
                airline_code: "X".into(),
                flight_no: "X 1".into(),
                aircraft: None,
                cabin: None,
                legroom: None,
                overnight: false,
                often_delayed: false,
                extras: vec![],
            }],
            layovers,
            total_min,
            price: Some(price),
            emissions_kg: None,
            emissions_vs_typical_pct: None,
            token: None,
        }
    }

    #[test]
    fn time_is_money() {
        let quick = itinerary(500.0, 8 * 60, vec![]);
        let slow = itinerary(480.0, 20 * 60, vec![]);
        assert!(
            deal_score(&quick, 1.0) < deal_score(&slow, 1.0),
            "$20 does not buy 12 extra hours"
        );
    }

    #[test]
    fn the_ranking_does_not_depend_on_the_currency() {
        // The same two trips priced in yen (150 to the dollar).
        let quick = itinerary(500.0 * 150.0, 8 * 60, vec![]);
        let slow = itinerary(480.0 * 150.0, 20 * 60, vec![]);
        assert!(deal_score(&quick, 150.0) < deal_score(&slow, 150.0));
    }

    #[test]
    fn unpriced_itineraries_sink() {
        let mut ghost = itinerary(0.0, 60, vec![]);
        ghost.price = None;
        assert!(deal_score(&ghost, 1.0) > deal_score(&itinerary(9000.0, 3000, vec![]), 1.0));
    }
}
