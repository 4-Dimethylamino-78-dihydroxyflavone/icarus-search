//! Background thread that owns the provider. The UI thread never blocks on
//! the network: it posts `Job`s and polls for `Reply`s once per frame.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
};

use chrono::NaiveDate;

use crate::{
    cache::Cache,
    model::{Query, SearchResult, Stage},
    provider::{Provider, ProviderError, Quota},
};

/// What a search is for, echoed back so the UI knows where to file it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// Outbound options for one departure date.
    Date(NaiveDate),
    /// Return options for the chosen outbound flight.
    Return,
}

pub enum Job {
    Search {
        generation: u64,
        purpose: Purpose,
        query: Query,
        stage: Stage,
    },
    /// The lowest fare for every departure date from `from` to `to`: one
    /// piece of a fare survey.
    Calendar {
        generation: u64,
        query: Query,
        from: NaiveDate,
        to: NaiveDate,
    },
    Quota,
    /// Replace the provider, e.g. after the user enters an API key.
    Swap(Box<dyn Provider>),
}

pub enum Reply {
    Search {
        generation: u64,
        purpose: Purpose,
        /// `Query::route_key` of the search, for the fare log.
        route: String,
        result: Result<SearchResult, ProviderError>,
        /// Served from disk, so it cost no API call.
        cached: bool,
    },
    Calendar {
        generation: u64,
        route: String,
        result: Result<Vec<(NaiveDate, f64)>, ProviderError>,
    },
    /// Fuel left - and, since asking is free, whether the key is accepted.
    Quota(Result<Option<Quota>, ProviderError>),
}

pub struct Worker {
    jobs: Sender<Job>,
    pub replies: Receiver<Reply>,
    /// The newest search generation. Queued jobs from older generations are
    /// skipped unsent: re-launching a scan must not burn quota on the old one.
    generation: Arc<AtomicU64>,
}

impl Worker {
    pub fn spawn(provider: Box<dyn Provider>, cache: Option<Cache>) -> Self {
        let (jobs, inbox) = mpsc::channel::<Job>();
        let (outbox, replies) = mpsc::channel::<Reply>();
        let generation = Arc::new(AtomicU64::new(0));
        let current = Arc::clone(&generation);
        thread::Builder::new()
            .name("icarus-worker".into())
            .spawn(move || run(provider, cache, inbox, outbox, current))
            .expect("the OS can spare one thread");
        Self {
            jobs,
            replies,
            generation,
        }
    }

    /// Start a new generation, invalidating every job still queued.
    pub fn next_generation(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn submit(&self, job: Job) {
        // The worker only stops when this handle is dropped, so a failed
        // send means we are shutting down anyway.
        let _ = self.jobs.send(job);
    }
}

fn run(
    mut provider: Box<dyn Provider>,
    cache: Option<Cache>,
    inbox: Receiver<Job>,
    outbox: Sender<Reply>,
    current: Arc<AtomicU64>,
) {
    for job in inbox {
        let reply = match job {
            Job::Swap(next) => {
                provider = next;
                continue;
            }
            Job::Quota => Reply::Quota(provider.quota()),
            Job::Calendar {
                generation,
                query,
                from,
                to,
            } => {
                if generation < current.load(Ordering::SeqCst) {
                    continue;
                }
                // Not cached on disk: every fare goes into the fare log,
                // which is what decides whether a range needs asking again.
                let route = query.route_key(provider.name());
                Reply::Calendar {
                    generation,
                    route,
                    result: provider.calendar(&query, from, to),
                }
            }
            Job::Search {
                generation,
                purpose,
                query,
                stage,
            } => {
                if generation < current.load(Ordering::SeqCst) {
                    continue;
                }
                // Simulated results are free to regenerate; only cache real ones.
                let cache = cache.as_ref().filter(|_| provider.is_live());
                let key = query.cache_key(provider.name(), &stage);
                let route = query.route_key(provider.name());
                let (result, cached) = match cache.and_then(|c| c.get(&key)) {
                    Some(hit) => (Ok(hit), true),
                    None => {
                        let result = provider.search(&query, &stage).map(|mut found| {
                            query.trim(&stage, &mut found);
                            found
                        });
                        if let (Ok(found), Some(c)) = (&result, cache) {
                            c.put(&key, found);
                        }
                        (result, false)
                    }
                };
                Reply::Search {
                    generation,
                    purpose,
                    route,
                    result,
                    cached,
                }
            }
        };
        if outbox.send(reply).is_err() {
            break; // UI is gone
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::{Cabin, Stops, TimeWindow},
        provider::demo::Demo,
    };
    use std::time::Duration;

    fn query() -> Query {
        let depart = chrono::Local::now().date_naive() + chrono::Duration::days(30);
        Query {
            origin: "LHR".into(),
            dest: "JFK".into(),
            depart,
            ret: None,
            adults: 1,
            cabin: Cabin::Economy,
            stops: Stops::Any,
            airlines: vec![],
            dep_window: TimeWindow::ANY,
            arr_window: TimeWindow::ANY,
            currency: "USD".into(),
        }
    }

    #[test]
    fn answers_current_jobs_and_skips_superseded_ones() {
        let worker = Worker::spawn(Box::new(Demo::new(false)), None);
        let q = query();
        let stale = worker.next_generation();
        let fresh = worker.next_generation();
        worker.submit(Job::Search {
            generation: stale,
            purpose: Purpose::Date(q.depart),
            query: q.clone(),
            stage: Stage::Outbound,
        });
        worker.submit(Job::Search {
            generation: fresh,
            purpose: Purpose::Date(q.depart),
            query: q.clone(),
            stage: Stage::Outbound,
        });
        worker.submit(Job::Quota);

        let first = worker
            .replies
            .recv_timeout(Duration::from_secs(10))
            .expect("a reply");
        match first {
            Reply::Search {
                generation,
                result,
                cached,
                ..
            } => {
                assert_eq!(generation, fresh, "the stale job was dropped, not answered");
                assert!(!cached);
                assert!(!result.unwrap().itineraries.is_empty());
            }
            _ => panic!("search reply expected first"),
        }
        assert!(matches!(
            worker.replies.recv_timeout(Duration::from_secs(10)),
            Ok(Reply::Quota(Ok(None)))
        ));
    }

    #[test]
    fn a_calendar_job_prices_a_range_and_dies_with_its_generation() {
        let worker = Worker::spawn(Box::new(Demo::new(false)), None);
        let q = query();
        let (from, to) = (q.depart, q.depart + chrono::Duration::days(9));
        let stale = worker.next_generation();
        let fresh = worker.next_generation();
        worker.submit(Job::Calendar {
            generation: stale,
            query: q.clone(),
            from,
            to,
        });
        worker.submit(Job::Calendar {
            generation: fresh,
            query: q.clone(),
            from,
            to,
        });
        match worker
            .replies
            .recv_timeout(Duration::from_secs(10))
            .expect("a reply")
        {
            Reply::Calendar {
                generation,
                route,
                result,
            } => {
                assert_eq!(generation, fresh);
                assert_eq!(
                    route,
                    q.route_key("SIMULATOR"),
                    "filed under the same route as a scan"
                );
                assert_eq!(result.unwrap().len(), 10);
            }
            _ => panic!("calendar reply expected"),
        }
    }
}
