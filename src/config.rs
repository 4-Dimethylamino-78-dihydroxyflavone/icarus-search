//! Settings, the remembered search, and API-key resolution.
//!
//! Everything a person might want to look at or change lives in one file,
//! `icarus-search.toml`, beside the executable: copy the two together and
//! the settings travel with the program. Where that folder cannot be written
//! (an installed copy, or macOS running a quarantined download from a
//! read-only mount) the same file is kept in the user's config folder.
//!
//! Key precedence follows the usual CLI rule, most specific wins:
//! `--key` flag > `ICARUS_API_KEY` env var > settings file > key baked in at
//! build time (`ICARUS_BAKED_KEY`, see README "Baking a key into a build").

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use chrono::NaiveDate;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::{
    currency,
    model::{Cabin, Stops, TimeWindow},
};

/// Set at compile time, never stored in the repository.
pub const BAKED_KEY: Option<&str> = option_env!("ICARUS_BAKED_KEY");

pub const FILE_NAME: &str = "icarus-search.toml";

const HEADER: &str = "\
# icarus-search settings. Safe to edit by hand while the program is closed;
# delete the file to start afresh.
#
#   currency    any 3-letter code, e.g. AUD USD EUR GBP NZD JPY
#   charset     auto | rich | safe   (safe avoids braille on old fonts)
#   theme       phosphor | amber
#   text_size   text height in pixels (Windows console only); 0 = leave it
#               alone. Smaller text gives a finer map. In the app: > and <
#   api_key     SerpApi key for live fares; empty runs the simulator
#   [mission]   the last search, restored next time

";

/// Hand-editable settings, plus the last search.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// SerpApi key. Empty = none.
    pub api_key: String,
    pub currency: String,
    /// "auto" | "rich" | "safe" - see `console` and `ui::theme::Glyphs`.
    pub charset: String,
    /// "phosphor" | "amber"
    pub theme: String,
    /// How long a cached search stays fresh.
    pub cache_hours: u32,
    /// Give each continent its own colour on the map.
    pub continents: bool,
    /// Draw country borders on the map.
    pub borders: bool,
    /// Text height in pixels on the classic Windows console, as last set
    /// with `>` and `<`; 0 leaves the console's own size alone. Smaller
    /// text fits more characters, and so a finer map.
    pub text_size: u16,
    /// Must stay the last field: TOML wants tables after plain values.
    pub mission: Session,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            currency: currency::DEFAULT.into(),
            charset: "auto".into(),
            theme: "phosphor".into(),
            cache_hours: 6,
            continents: true,
            borders: true,
            text_size: 0,
            mission: Session::default(),
        }
    }
}

/// Where the last run left off, restored on start-up.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub origin: String,
    pub dest: String,
    pub round_trip: bool,
    pub depart: Option<NaiveDate>,
    pub ret: Option<NaiveDate>,
    /// Days of date flexibility: either side of the departure date, or...
    pub flex: u8,
    /// ...only after it.
    pub flex_later: bool,
    pub cabin: Cabin,
    pub stops: Stops,
    pub adults: u8,
    pub airlines: Vec<String>,
    pub dep_window: TimeWindow,
    pub arr_window: TimeWindow,
}

/// What a first-time user sees: a round trip for one, comparing a week of
/// dates around the chosen one.
impl Default for Session {
    fn default() -> Self {
        Self {
            origin: String::new(),
            dest: String::new(),
            round_trip: true,
            depart: None,
            ret: None,
            flex: 3,
            flex_later: false,
            cabin: Cabin::default(),
            stops: Stops::default(),
            adults: 1,
            airlines: Vec::new(),
            dep_window: TimeWindow::ANY,
            arr_window: TimeWindow::ANY,
        }
    }
}

/// Bulk data, which stays out of the way in the user's own folders.
pub struct Paths {
    /// Long-lived log of every fare sighted (see `farelog`).
    pub fares: PathBuf,
    pub cache: PathBuf,
}

pub fn paths() -> Option<Paths> {
    let dirs = ProjectDirs::from("", "", "icarus-search")?;
    Some(Paths {
        fares: dirs.data_local_dir().join("fares.jsonl"),
        cache: dirs.cache_dir().join("searches"),
    })
}

/// Places the settings file may live, in order of preference.
fn candidates() -> Vec<PathBuf> {
    let beside_exe = env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join(FILE_NAME)));
    let in_profile =
        ProjectDirs::from("", "", "icarus-search").map(|d| d.config_dir().join(FILE_NAME));
    beside_exe.into_iter().chain(in_profile).collect()
}

/// Settings written by the first version, which kept them in two files in
/// the user's profile. Either file may be missing.
fn legacy() -> Option<Config> {
    let dirs = ProjectDirs::from("", "", "icarus-search")?;
    let old_settings: Option<Config> = fs::read_to_string(dirs.config_dir().join("config.toml"))
        .ok()
        .and_then(|text| toml::from_str(&text).ok());
    let old_session: Option<Session> =
        fs::read_to_string(dirs.data_local_dir().join("session.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok());
    if old_settings.is_none() && old_session.is_none() {
        return None;
    }
    let mut cfg = old_settings.unwrap_or_default();
    // That version's currency was a default nobody chose (US dollars), so
    // it is not carried over as if it were a preference.
    cfg.currency = currency::DEFAULT.into();
    if let Some(session) = old_session {
        cfg.mission = session;
    }
    Some(cfg)
}

impl Config {
    /// Load the settings, and say so if a damaged file had to be set aside.
    pub fn load() -> (Self, Option<String>) {
        for path in candidates() {
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            return match Self::parse(&text) {
                Ok(cfg) => (cfg, None),
                Err(_) => {
                    // Keep the damaged file: overwriting someone's hand
                    // edits because of one typo would be unforgivable.
                    let aside = path.with_extension("toml.bad");
                    let _ = fs::rename(&path, &aside);
                    (
                        Self::default(),
                        Some(format!(
                            "{FILE_NAME} COULD NOT BE READ. DEFAULTS LOADED; THE OLD FILE IS KEPT AS .BAD"
                        )),
                    )
                }
            };
        }
        (legacy().unwrap_or_default(), None)
    }

    fn parse(text: &str) -> Result<Self, String> {
        let mut cfg: Config = toml::from_str(text).map_err(|e| e.to_string())?;
        cfg.currency =
            currency::normalize(&cfg.currency).unwrap_or_else(|| currency::DEFAULT.into());
        Ok(cfg)
    }

    fn render(&self) -> Result<String, String> {
        Ok(format!(
            "{HEADER}{}",
            toml::to_string_pretty(self).map_err(|e| e.to_string())?
        ))
    }

    /// Write the settings to the first place that accepts them, returning
    /// where they went.
    pub fn save(&self) -> Result<PathBuf, String> {
        let text = self.render()?;
        let mut last_error = "no home directory".to_string();
        for path in candidates() {
            match write_file(&path, &text) {
                Ok(()) => return Ok(path),
                Err(e) => last_error = e,
            }
        }
        Err(last_error)
    }
}

fn write_file(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(path, text).map_err(|e| e.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeySource {
    Flag,
    Env,
    File,
    Baked,
}

pub fn resolve_key(flag: Option<&str>, cfg: &Config) -> Option<(String, KeySource)> {
    fn clean(s: &str) -> Option<String> {
        let s = s.trim();
        (!s.is_empty()).then(|| s.to_string())
    }
    flag.and_then(clean)
        .map(|k| (k, KeySource::Flag))
        .or_else(|| {
            env::var("ICARUS_API_KEY")
                .ok()
                .as_deref()
                .and_then(clean)
                .map(|k| (k, KeySource::Env))
        })
        .or_else(|| clean(&cfg.api_key).map(|k| (k, KeySource::File)))
        .or_else(|| BAKED_KEY.and_then(clean).map(|k| (k, KeySource::Baked)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_beats_file_and_blank_counts_as_absent() {
        let cfg = Config {
            api_key: "from-file".into(),
            ..Config::default()
        };
        assert_eq!(
            resolve_key(Some("from-flag"), &cfg),
            Some(("from-flag".into(), KeySource::Flag))
        );
        // Only meaningful when the env var is unset, as it is under `cargo test`.
        if env::var("ICARUS_API_KEY").is_err() {
            assert_eq!(
                resolve_key(Some("  "), &cfg),
                Some(("from-file".into(), KeySource::File))
            );
        }
    }

    #[test]
    fn settings_survive_a_round_trip_through_the_file() {
        let mission = Session {
            origin: "SYD".into(),
            dest: "SGN".into(),
            round_trip: false,
            depart: NaiveDate::from_ymd_opt(2026, 12, 15),
            dep_window: TimeWindow(Some((360, 719))),
            airlines: vec!["QF".into(), "VN".into()],
            adults: 2,
            ..Session::default()
        };
        let cfg = Config {
            api_key: "k".into(),
            currency: "EUR".into(),
            mission,
            ..Config::default()
        };
        let text = cfg.render().unwrap();
        assert!(text.starts_with("# icarus-search settings"), "{text}");
        let back = Config::parse(&text).unwrap();
        assert_eq!(
            (back.api_key.as_str(), back.currency.as_str()),
            ("k", "EUR")
        );
        assert_eq!(
            (back.mission.origin.as_str(), back.mission.dest.as_str()),
            ("SYD", "SGN")
        );
        assert!(!back.mission.round_trip);
        assert_eq!(back.mission.depart, NaiveDate::from_ymd_opt(2026, 12, 15));
        assert_eq!(
            back.mission.ret, None,
            "an unset date is simply absent from the file"
        );
        assert_eq!(back.mission.dep_window, TimeWindow(Some((360, 719))));
        assert_eq!(back.mission.arr_window, TimeWindow::ANY);
        assert_eq!(back.mission.airlines, ["QF", "VN"]);
        assert_eq!(back.mission.adults, 2);
    }

    #[test]
    fn missing_and_odd_values_fall_back_to_defaults() {
        let partial = Config::parse("currency = \"gbp\"").unwrap();
        assert_eq!(partial.currency, "GBP");
        assert_eq!(partial.cache_hours, 6);
        assert!(partial.mission.round_trip);
        assert_eq!(
            Config::parse("currency = \"dollars\"").unwrap().currency,
            currency::DEFAULT
        );
        assert_eq!(Config::default().currency, "AUD");
        assert!(
            Config::parse("currency = ").is_err(),
            "a broken file is reported, not guessed at"
        );
    }

    #[test]
    fn the_settings_file_sits_beside_the_executable() {
        let first = candidates()
            .into_iter()
            .next()
            .expect("the test binary has a path");
        assert_eq!(first.file_name().and_then(|n| n.to_str()), Some(FILE_NAME));
        assert_eq!(first.parent(), env::current_exe().unwrap().parent());
    }
}
