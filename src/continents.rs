//! Which continent a coordinate belongs to, so the map can give each its
//! own colour.
//!
//! This is a handful of straight boundary lines, not a geographic dataset:
//! the Urals, the Caucasus, the Bosphorus, the Red Sea, Panama, the Wallace
//! line. That is accurate to a few tens of kilometres along each seam, which
//! is finer than one character of map at any zoom the app reaches. It only
//! has to be right on land - it is asked about dots the land mask has
//! already said yes to.

use crate::geo::LatLon;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Continent {
    Africa,
    Antarctica,
    Asia,
    Europe,
    NorthAmerica,
    Oceania,
    SouthAmerica,
}

impl Continent {
    pub const COUNT: usize = 7;

    pub fn index(self) -> usize {
        self as usize
    }
}

pub fn of(p: LatLon) -> Continent {
    use Continent::*;
    let (lat, lon) = (p.lat, p.lon);
    if lat < -60.0 {
        return Antarctica;
    }
    if lon < -32.0 {
        // The far north-east of Russia reaches across the date line.
        if lon < -168.5 && lat > 62.0 {
            return Asia;
        }
        // Hawaii and Polynesia.
        if lon < -130.0 && lat < 30.0 {
            return Oceania;
        }
        // The two Americas part at the Panama-Colombia border.
        let south = lat < 7.0 || (lat < 12.5 && lon > -77.5);
        return if south { SouthAmerica } else { NorthAmerica };
    }
    // Greenland reaches east to 11 W; Iceland sits inside the same box.
    if lon < -10.0 && lat > 59.0 {
        let iceland = (63.0..67.0).contains(&lat) && lon > -25.0;
        return if iceland { Europe } else { NorthAmerica };
    }
    // Australia and New Zealand; New Guinea and Melanesia; Micronesia.
    if (lat < -10.6 && lon > 110.0) || (lat < 0.0 && lon > 130.5) || (lat < 20.0 && lon > 133.0) {
        return Oceania;
    }
    if lon < 60.0 && africa(lat, lon) {
        return Africa;
    }
    if europe(lat, lon) { Europe } else { Asia }
}

/// East of 32 W and west of 60 E only.
fn africa(lat: f64, lon: f64) -> bool {
    if lat < 0.0 {
        return true; // as far east as Mauritius
    }
    if lat <= 12.0 {
        return lon < 55.0; // the Horn; Yemen's coast is all north of this
    }
    if lat < 30.0 {
        // West of the Red Sea, taken as a straight line from the
        // Bab-el-Mandeb (12.6 N, 43.3 E) to Suez (30 N, 32.55 E).
        return lon < 43.3 - 0.618 * (lat - 12.6);
    }
    // North of 30 N: the southern shore of the Mediterranean, west to east,
    // and nothing beyond the Suez Canal.
    let shore = if lon < -1.0 {
        35.95 // the Strait of Gibraltar
    } else if lon < 7.5 {
        37.4
    } else if lon < 11.5 {
        37.6 // Tunisia's northern tip, short of Sardinia and Sicily
    } else if lon < 25.0 {
        34.0 // Libya, short of Malta and Crete
    } else {
        33.0
    };
    lat < shore && lon < 32.5
}

/// East of 32 W, and not Africa.
fn europe(lat: f64, lon: f64) -> bool {
    let eastern_edge = if lat >= 73.0 {
        70.0 // Novaya Zemlya
    } else if lat >= 70.0 {
        62.0 // ...but not the Yamal peninsula beside it
    } else if lat >= 64.0 {
        66.0 // the Polar Urals
    } else if lat >= 51.5 {
        60.0 // the Urals
    } else if lat >= 47.0 {
        51.7 // the Ural river, down to the Caspian
    } else if lat >= 44.5 {
        49.0 // the Caspian's western shore
    } else if lat >= 43.2 {
        47.8 // north of the Caucasus watershed
    } else if lat >= 40.6 {
        29.1 // the Bosphorus
    } else if lat >= 36.0 {
        26.1 // the Aegean
    } else if lat >= 34.0 {
        // Only Malta and Crete lie this far south.
        return lon > 11.5 && lon < 26.5;
    } else {
        return false;
    };
    lon < eastern_edge
}

#[cfg(test)]
mod tests {
    use super::Continent::*;
    use super::*;
    use crate::airports;

    #[test]
    fn cities_either_side_of_each_seam() {
        let cases = [
            ("London", 51.5, -0.1, Europe),
            ("Reykjavik", 64.1, -21.9, Europe),
            ("Nuuk", 64.2, -51.7, NorthAmerica),
            ("Lisbon", 38.8, -9.1, Europe),
            ("Gibraltar", 36.14, -5.35, Europe),
            ("Tangier", 35.77, -5.8, Africa),
            ("Algiers", 36.7, 3.2, Africa),
            ("Tunis", 36.85, 10.2, Africa),
            ("Palermo", 38.1, 13.4, Europe),
            ("Malta", 35.9, 14.5, Europe),
            ("Tripoli", 32.7, 13.2, Africa),
            ("Heraklion", 35.3, 25.1, Europe),
            ("Cairo", 30.1, 31.4, Africa),
            ("Tel Aviv", 32.0, 34.9, Asia),
            ("Port Sudan", 19.6, 37.2, Africa),
            ("Jeddah", 21.7, 39.2, Asia),
            ("Asmara", 15.3, 38.9, Africa),
            ("Aden", 12.8, 45.0, Asia),
            ("Djibouti", 11.5, 43.1, Africa),
            ("Mauritius", -20.3, 57.5, Africa),
            ("Istanbul Airport", 41.26, 28.74, Europe),
            ("Sabiha Gokcen", 40.9, 29.3, Asia),
            ("Antalya", 36.9, 30.8, Asia),
            ("Athens", 37.9, 23.9, Europe),
            ("Tbilisi", 41.7, 44.9, Asia),
            ("Sochi", 43.4, 39.9, Europe),
            ("Baku", 40.4, 49.9, Asia),
            ("Moscow", 55.8, 37.6, Europe),
            ("Perm", 58.0, 56.2, Europe),
            ("Yekaterinburg", 56.8, 60.6, Asia),
            ("Aktobe", 50.3, 57.2, Asia),
            ("Anadyr", 64.7, 177.5, Asia),
            ("Uelen, across the date line", 66.2, -169.8, Asia),
            ("Anchorage", 61.2, -150.0, NorthAmerica),
            ("Honolulu", 21.3, -157.9, Oceania),
            ("Panama City", 9.0, -79.5, NorthAmerica),
            ("Bogota", 4.7, -74.1, SouthAmerica),
            ("Caracas", 10.6, -66.9, SouthAmerica),
            ("Jakarta", -6.1, 106.8, Asia),
            ("Denpasar", -8.7, 115.2, Asia),
            ("Dili", -8.6, 125.6, Asia),
            ("Darwin", -12.4, 130.9, Oceania),
            ("Port Moresby", -9.4, 147.2, Oceania),
            ("Sydney", -33.9, 151.2, Oceania),
            ("Auckland", -37.0, 174.8, Oceania),
            ("Guam", 13.5, 144.8, Oceania),
            ("Tokyo", 35.6, 139.8, Asia),
            ("McMurdo", -77.8, 166.7, Antarctica),
        ];
        for (name, lat, lon, expected) in cases {
            assert_eq!(of(LatLon::new(lat, lon)), expected, "{name}");
        }
    }

    /// Every airport of a country that lies wholly within one continent
    /// must land on that continent: a few thousand real coordinates.
    #[test]
    fn airports_agree_with_their_countries() {
        let wholly = [
            (
                Europe,
                &[
                    "DE", "PL", "CH", "AT", "CZ", "HU", "RO", "BG", "SE", "FI", "IE", "GB", "FR",
                    "IT", "UA", "NL", "BE",
                ][..],
            ),
            (
                Asia,
                &[
                    "JP", "CN", "IN", "TH", "VN", "KR", "SA", "IR", "PK", "MN", "MY", "PH", "AE",
                    "IL",
                ][..],
            ),
            (
                Africa,
                &[
                    "NG", "KE", "ZA", "ET", "DZ", "MA", "TN", "LY", "TZ", "GH", "SN", "AO", "CD",
                    "SD",
                ][..],
            ),
            (NorthAmerica, &["CA", "MX", "CU", "GT", "CR"][..]),
            (
                SouthAmerica,
                &["BR", "AR", "CL", "PE", "BO", "PY", "UY", "EC", "VE"][..],
            ),
            (Oceania, &["AU", "NZ", "PG", "FJ"][..]),
        ];
        let mut checked = 0;
        for airport in airports::db().all() {
            for (continent, countries) in wholly {
                if countries.contains(&airport.country) {
                    assert_eq!(
                        of(airport.pos),
                        continent,
                        "{} ({})",
                        airport.long(),
                        airport.country
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 1500, "only {checked} airports checked");
    }
}
