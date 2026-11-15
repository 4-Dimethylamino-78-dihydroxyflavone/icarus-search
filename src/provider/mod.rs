//! Flight data sources. The rest of the app only ever sees this trait.

pub mod demo;
pub mod serpapi;

use std::fmt;

use chrono::NaiveDate;

use crate::model::{Query, SearchResult, Stage};

#[derive(Clone, Debug, PartialEq)]
pub enum ProviderError {
    /// The API key was rejected.
    Auth(String),
    /// Out of searches, or rate limited.
    Quota(String),
    /// Could not reach the service.
    Network(String),
    /// The service answered with an error of its own.
    Api(String),
    /// The service answered with something we could not read.
    Parse(String),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auth(m) => write!(f, "API KEY REJECTED: {m}"),
            Self::Quota(m) => write!(f, "OUT OF FUEL: {m}"),
            Self::Network(m) => write!(f, "NO UPLINK: {m}"),
            Self::Api(m) => write!(f, "SOURCE ERROR: {m}"),
            Self::Parse(m) => write!(f, "UNREADABLE REPLY: {m}"),
        }
    }
}

/// Searches remaining on the account ("fuel").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quota {
    pub left: u32,
    pub per_month: u32,
}

/// The most days one `Provider::calendar` call may span. Google Flights
/// answers a price calendar about two months at a time.
pub const CALENDAR_DAYS: i64 = 61;

/// `Send` because the provider lives on the worker thread, never the UI thread.
pub trait Provider: Send {
    fn name(&self) -> &'static str;

    /// `false` for simulated data; the UI labels it loudly.
    fn is_live(&self) -> bool;

    /// Finding no flights is `Ok` with an empty list, not an error.
    fn search(&mut self, query: &Query, stage: &Stage) -> Result<SearchResult, ProviderError>;

    /// The lowest fare for each departure date from `from` to `to`
    /// inclusive (at most `CALENDAR_DAYS` of them), in one call. The
    /// query's own departure date is not used; a round trip keeps its
    /// length. Dates with nothing on sale are simply absent.
    ///
    /// This is what makes a year's heatmap affordable: six calls, where
    /// searching each day would take hundreds.
    fn calendar(
        &mut self,
        query: &Query,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<(NaiveDate, f64)>, ProviderError>;

    /// Searches left on the account, where there is one. Costs nothing to
    /// ask, so it doubles as the check that a key is accepted: a rejected
    /// key is an `Err`.
    fn quota(&mut self) -> Result<Option<Quota>, ProviderError> {
        Ok(None)
    }
}
