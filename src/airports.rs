//! Embedded airport database (OurAirports, public domain): every airport with
//! an IATA code and scheduled service. Loaded once, handed out as `&'static`.

use std::{collections::HashMap, sync::OnceLock};

use crate::geo::{LatLon, V3};

static RAW: &str = include_str!("../data/airports.tsv");

#[derive(Debug)]
pub struct Airport {
    pub iata: &'static str,
    /// 0 = large, 1 = medium, 2 = small.
    pub size: u8,
    pub pos: LatLon,
    /// `pos` as a unit vector, cached for nearest-airport searches.
    pub vec: V3,
    pub country: &'static str,
    pub city: &'static str,
    pub name: &'static str,
    /// Search-only aliases such as metro codes ("TYO") and former names.
    aliases: &'static str,
}

impl Airport {
    /// "LHR LONDON"
    pub fn short(&self) -> String {
        format!("{} {}", self.iata, self.city.to_uppercase())
    }

    /// "LHR LONDON - LONDON HEATHROW (GB)"
    pub fn long(&self) -> String {
        format!(
            "{} {} - {} ({})",
            self.iata,
            self.city.to_uppercase(),
            self.name.to_uppercase(),
            self.country
        )
    }
}

pub struct AirportDb {
    all: Vec<Airport>,
    index: HashMap<&'static str, usize>,
}

pub fn db() -> &'static AirportDb {
    static DB: OnceLock<AirportDb> = OnceLock::new();
    DB.get_or_init(AirportDb::load)
}

impl AirportDb {
    fn load() -> Self {
        let mut all = Vec::with_capacity(4200);
        for line in RAW.lines() {
            let mut cols = line.split('\t');
            let (
                Some(iata),
                Some(size),
                Some(lat),
                Some(lon),
                Some(country),
                Some(city),
                Some(name),
            ) = (
                cols.next(),
                cols.next(),
                cols.next(),
                cols.next(),
                cols.next(),
                cols.next(),
                cols.next(),
            )
            else {
                continue;
            };
            let (Ok(size), Ok(lat), Ok(lon)) = (size.parse(), lat.parse(), lon.parse()) else {
                continue;
            };
            let pos = LatLon::new(lat, lon);
            all.push(Airport {
                iata,
                size,
                pos,
                vec: pos.to_vec(),
                country,
                city,
                name,
                aliases: cols.next().unwrap_or(""),
            });
        }
        let index = all.iter().enumerate().map(|(i, a)| (a.iata, i)).collect();
        Self { all, index }
    }

    pub fn all(&self) -> &[Airport] {
        &self.all
    }

    pub fn get(&self, iata: &str) -> Option<&Airport> {
        self.index
            .get(iata.trim().to_ascii_uppercase().as_str())
            .map(|&i| &self.all[i])
    }

    /// Free-text lookup by IATA code, city, airport name or alias.
    /// Best matches first; bigger airports win ties.
    pub fn search(&self, query: &str, limit: usize) -> Vec<&Airport> {
        let q = query.trim().to_ascii_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let mut hits: Vec<(u8, &Airport)> = self
            .all
            .iter()
            .filter_map(|a| rank(a, &q).map(|r| (r, a)))
            .collect();
        hits.sort_by(|x, y| {
            (x.0, x.1.size, x.1.city, x.1.iata).cmp(&(y.0, y.1.size, y.1.city, y.1.iata))
        });
        hits.into_iter().take(limit).map(|(_, a)| a).collect()
    }
}

/// Lower is better; `None` means no match.
fn rank(a: &Airport, q: &str) -> Option<u8> {
    let iata = a.iata.to_ascii_lowercase();
    if iata == q {
        return Some(0);
    }
    let city = a.city.to_ascii_lowercase();
    if city.starts_with(q) {
        return Some(1);
    }
    if q.len() < 3 && iata.starts_with(q) {
        return Some(2);
    }
    let name = a.name.to_ascii_lowercase();
    if word_prefix(&name, q) {
        return Some(3);
    }
    if word_prefix(&a.aliases.to_ascii_lowercase(), q) {
        return Some(4);
    }
    (city.contains(q) || name.contains(q)).then_some(5)
}

/// Does `q` appear in `text` at the start of a word?
fn word_prefix(text: &str, q: &str) -> bool {
    text.match_indices(q)
        .any(|(i, _)| i == 0 || !text.as_bytes()[i - 1].is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_loads_and_indexes() {
        let db = db();
        assert!(db.all().len() > 3500);
        let lhr = db.get("lhr").expect("LHR present");
        assert_eq!(lhr.city, "London");
        assert_eq!(lhr.size, 0);
        assert!(db.get("ZZZ").is_none());
    }

    #[test]
    fn city_search_finds_every_airport_serving_it() {
        let codes: Vec<_> = db().search("tokyo", 10).iter().map(|a| a.iata).collect();
        assert!(codes.contains(&"HND"), "{codes:?}");
        assert!(
            codes.contains(&"NRT"),
            "NRT is found through its TYO/Tokyo alias: {codes:?}"
        );
    }

    #[test]
    fn exact_code_ranks_first() {
        assert_eq!(db().search("jfk", 5)[0].iata, "JFK");
        assert_eq!(db().search("  CDG ", 5)[0].iata, "CDG");
    }
}
