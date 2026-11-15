//! Text formatting shared by every panel. All times are 24h "military" HHMM.

use chrono::{NaiveDate, NaiveDateTime};

use crate::currency;

/// "16H25M", "45M"
pub fn duration(min: u32) -> String {
    if min < 60 {
        format!("{min}M")
    } else {
        format!("{}H{:02}M", min / 60, min % 60)
    }
}

/// "2H10" - compact form for table cells.
pub fn duration_short(min: u32) -> String {
    format!("{}H{:02}", min / 60, min % 60)
}

/// "0915"
pub fn hhmm(t: NaiveDateTime) -> String {
    t.format("%H%M").to_string()
}

/// "2026-12-10 0915"
pub fn stamp(t: NaiveDateTime) -> String {
    t.format("%Y-%m-%d %H%M").to_string()
}

/// "2026-12-10 THU"
pub fn day(d: NaiveDate) -> String {
    d.format("%Y-%m-%d %a").to_string().to_uppercase()
}

/// "THU 12-10"
pub fn day_short(d: NaiveDate) -> String {
    d.format("%a %m-%d").to_string().to_uppercase()
}

/// "9,560"
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// "A$1,742", "CHF 1,742", "VND 12.3M" - whole units, which is all a fare
/// hunt needs. Seven-figure amounts (dong, rupiah, won) are abbreviated so
/// they still fit a table column.
pub fn money(value: f64, code: &str) -> String {
    let symbol = currency::get(code).map_or(code, |c| c.symbol);
    let value = value.round().max(0.0);
    let amount = if value >= 100_000_000.0 {
        format!("{:.0}M", value / 1e6)
    } else if value >= 10_000_000.0 {
        format!("{:.1}M", value / 1e6)
    } else if value >= 1_000_000.0 {
        format!("{:.2}M", value / 1e6)
    } else {
        thousands(value as u64)
    };
    // "RM 1,250" needs the space; "A$1,250" does not.
    let gap = if symbol.ends_with(|ch: char| ch.is_ascii_alphabetic()) {
        " "
    } else {
        ""
    };
    format!("{symbol}{gap}{amount}")
}

/// Truncate to `width` characters, ending in `ellipsis` if anything was cut.
pub fn clip(text: &str, width: usize, ellipsis: &str) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let keep = width.saturating_sub(ellipsis.chars().count());
    let mut out: String = text.chars().take(keep).collect();
    out.push_str(ellipsis);
    out.chars().take(width).collect()
}

/// Left-align in exactly `width` characters.
pub fn pad(text: &str, width: usize, ellipsis: &str) -> String {
    format!("{:<width$}", clip(text, width, ellipsis))
}

/// Right-align in exactly `width` characters.
pub fn pad_left(text: &str, width: usize, ellipsis: &str) -> String {
    format!("{:>width$}", clip(text, width, ellipsis))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(duration(45), "45M");
        assert_eq!(duration(985), "16H25M");
        assert_eq!(duration_short(130), "2H10");
    }

    #[test]
    fn money_and_grouping() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(money(1741.6, "USD"), "$1,742");
        assert_eq!(money(1741.6, "AUD"), "A$1,742");
        assert_eq!(money(88.0, "CHF"), "CHF 88");
        assert_eq!(
            money(88.0, "ISK"),
            "ISK 88",
            "an unlisted code is its own prefix"
        );
        // Every amount fits the 9-character price column.
        assert_eq!(money(12_345_678.0, "VND"), "VND 12.3M");
        assert_eq!(money(1_250_300.0, "JPY"), "¥1.25M");
        assert_eq!(money(999_999.0, "JPY"), "¥999,999");
        assert_eq!(money(250_000_000.0, "IDR"), "RP 250M");
    }

    #[test]
    fn clipping_and_padding() {
        assert_eq!(clip("QATAR AIRWAYS", 8, "~"), "QATAR A~");
        assert_eq!(pad("AB", 4, "~"), "AB  ");
        assert_eq!(pad_left("AB", 4, "~"), "  AB");
    }
}
