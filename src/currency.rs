//! Currencies the picker offers, how each is written, and a rough exchange
//! rate for the simulator.
//!
//! Fares are quotes to compare, not money being moved: they stay `f64`, are
//! shown in whole units, and are never converted between currencies. A fare
//! fetched in one currency is filed under that currency (`Query::route_key`)
//! and never shares a chart with a fare fetched in another.

pub const DEFAULT: &str = "AUD";

pub struct Currency {
    pub code: &'static str,
    /// Prefix for amounts. Kept to characters every console font can draw.
    pub symbol: &'static str,
    pub name: &'static str,
    /// Units per US dollar, to the nearest sensible figure. Only the
    /// simulator and the BEST ranking use it, to keep numbers plausible;
    /// live fares come from the provider already in the right currency.
    pub per_usd: f64,
}

const fn c(code: &'static str, symbol: &'static str, name: &'static str, per_usd: f64) -> Currency {
    Currency {
        code,
        symbol,
        name,
        per_usd,
    }
}

/// Most-reached-for first; the picker lists them in this order.
pub const ALL: [Currency; 36] = [
    c("AUD", "A$", "Australian dollar", 1.52),
    c("USD", "$", "US dollar", 1.0),
    c("EUR", "€", "Euro", 0.92),
    c("GBP", "£", "Pound sterling", 0.79),
    c("NZD", "NZ$", "New Zealand dollar", 1.66),
    c("CAD", "C$", "Canadian dollar", 1.37),
    c("JPY", "¥", "Japanese yen", 150.0),
    c("SGD", "S$", "Singapore dollar", 1.34),
    c("HKD", "HK$", "Hong Kong dollar", 7.8),
    c("CNY", "CN¥", "Chinese yuan", 7.2),
    c("INR", "RS", "Indian rupee", 84.0),
    c("IDR", "RP", "Indonesian rupiah", 16_000.0),
    c("THB", "THB", "Thai baht", 35.0),
    c("VND", "VND", "Vietnamese dong", 25_000.0),
    c("MYR", "RM", "Malaysian ringgit", 4.5),
    c("PHP", "PHP", "Philippine peso", 57.0),
    c("KRW", "KRW", "South Korean won", 1_380.0),
    c("TWD", "NT$", "New Taiwan dollar", 32.0),
    c("FJD", "FJ$", "Fijian dollar", 2.25),
    c("CHF", "CHF", "Swiss franc", 0.88),
    c("SEK", "SEK", "Swedish krona", 10.5),
    c("NOK", "NOK", "Norwegian krone", 10.8),
    c("DKK", "DKK", "Danish krone", 6.9),
    c("PLN", "PLN", "Polish zloty", 4.0),
    c("CZK", "CZK", "Czech koruna", 23.0),
    c("HUF", "HUF", "Hungarian forint", 360.0),
    c("TRY", "TRY", "Turkish lira", 34.0),
    c("ILS", "ILS", "Israeli shekel", 3.7),
    c("AED", "AED", "UAE dirham", 3.67),
    c("SAR", "SAR", "Saudi riyal", 3.75),
    c("QAR", "QAR", "Qatari riyal", 3.64),
    c("EGP", "EGP", "Egyptian pound", 49.0),
    c("ZAR", "R", "South African rand", 18.0),
    c("BRL", "R$", "Brazilian real", 5.5),
    c("MXN", "MX$", "Mexican peso", 18.5),
    c("ARS", "ARS", "Argentine peso", 1_000.0),
];

pub fn get(code: &str) -> Option<&'static Currency> {
    ALL.iter().find(|c| c.code.eq_ignore_ascii_case(code))
}

/// Units of `code` per US dollar; 1.0 for a currency not in the table.
pub fn per_usd(code: &str) -> f64 {
    get(code).map_or(1.0, |c| c.per_usd)
}

/// The country whose Google Flights someone paying in `code` most likely
/// shops on, as a two-letter code. Fares and sellers differ by country, so
/// asking as that country gives what they would see in their own browser.
/// `None` where a currency does not point at one country (the euro), and
/// the source's own default is left alone.
pub fn market(code: &str) -> Option<&'static str> {
    const HOME: [(&str, &str); 35] = [
        ("AUD", "au"),
        ("USD", "us"),
        ("GBP", "uk"),
        ("NZD", "nz"),
        ("CAD", "ca"),
        ("JPY", "jp"),
        ("SGD", "sg"),
        ("HKD", "hk"),
        ("CNY", "cn"),
        ("INR", "in"),
        ("IDR", "id"),
        ("THB", "th"),
        ("VND", "vn"),
        ("MYR", "my"),
        ("PHP", "ph"),
        ("KRW", "kr"),
        ("TWD", "tw"),
        ("FJD", "fj"),
        ("CHF", "ch"),
        ("SEK", "se"),
        ("NOK", "no"),
        ("DKK", "dk"),
        ("PLN", "pl"),
        ("CZK", "cz"),
        ("HUF", "hu"),
        ("TRY", "tr"),
        ("ILS", "il"),
        ("AED", "ae"),
        ("SAR", "sa"),
        ("QAR", "qa"),
        ("EGP", "eg"),
        ("ZAR", "za"),
        ("BRL", "br"),
        ("MXN", "mx"),
        ("ARS", "ar"),
    ];
    HOME.iter()
        .find(|(currency, _)| currency.eq_ignore_ascii_case(code))
        .map(|(_, country)| *country)
}

/// A code the provider could be asked for: three ASCII letters.
pub fn normalize(text: &str) -> Option<String> {
    let code = text.trim().to_ascii_uppercase();
    (code.len() == 3 && code.bytes().all(|b| b.is_ascii_uppercase())).then_some(code)
}

/// Currencies matching what has been typed, by code first and then by name.
pub fn search(input: &str) -> Vec<&'static Currency> {
    let needle = input.trim().to_ascii_uppercase();
    if needle.is_empty() {
        return ALL.iter().collect();
    }
    let by_code = ALL.iter().filter(|c| c.code.starts_with(&needle));
    let by_name = ALL
        .iter()
        .filter(|c| !c.code.starts_with(&needle) && c.name.to_ascii_uppercase().contains(&needle));
    by_code.chain(by_name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique_and_well_formed() {
        for (i, a) in ALL.iter().enumerate() {
            assert_eq!(normalize(a.code).as_deref(), Some(a.code));
            assert!(a.per_usd > 0.0);
            assert!(
                ALL[i + 1..].iter().all(|b| b.code != a.code),
                "{} is listed twice",
                a.code
            );
        }
        assert_eq!(ALL[0].code, DEFAULT);
    }

    #[test]
    fn lookup_and_search() {
        assert_eq!(get("aud").map(|c| c.symbol), Some("A$"));
        assert!(get("XXX").is_none());
        assert_eq!(per_usd("XXX"), 1.0);
        assert_eq!(search("").len(), ALL.len());
        assert_eq!(search("nz").first().map(|c| c.code), Some("NZD"));
        // A name matches too, after any code matches.
        assert_eq!(search("dollar").first().map(|c| c.code), Some("AUD"));
        assert!(search("yen").iter().any(|c| c.code == "JPY"));
        assert!(search("zzz").is_empty());
    }

    #[test]
    fn a_currency_points_at_its_home_market_when_it_has_one() {
        assert_eq!(market("AUD"), Some("au"));
        assert_eq!(market("gbp"), Some("uk"));
        assert_eq!(market("EUR"), None, "the euro is nobody's alone");
        assert_eq!(market("ISK"), None, "a typed code outside the table");
        // Every listed currency but the euro has a home.
        for c in ALL.iter().filter(|c| c.code != "EUR") {
            assert!(market(c.code).is_some(), "{} has no market", c.code);
        }
    }

    #[test]
    fn typed_codes_are_validated() {
        assert_eq!(normalize(" isk ").as_deref(), Some("ISK"));
        assert_eq!(normalize("US"), None);
        assert_eq!(normalize("U$D"), None);
        assert_eq!(normalize("EURO"), None);
    }
}
