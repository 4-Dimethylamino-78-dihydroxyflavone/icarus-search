//! Disk cache for live search results. Free API tiers are small, so the same
//! question is never paid for twice within the time-to-live.

use std::{fs, path::PathBuf, time::SystemTime};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::model::SearchResult;

/// FNV-1a: tiny, dependency-free, and (unlike `DefaultHasher`) guaranteed
/// stable across Rust versions, which matters for file names on disk.
pub fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

pub struct Cache {
    dir: PathBuf,
    ttl_secs: i64,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    at: i64,
    /// Stored in full so a hash collision can never serve the wrong route.
    key: String,
    result: SearchResult,
}

impl Cache {
    pub fn open(dir: PathBuf, ttl_hours: u32) -> Option<Self> {
        fs::create_dir_all(&dir).ok()?;
        let cache = Self {
            dir,
            ttl_secs: i64::from(ttl_hours.max(1)) * 3600,
        };
        cache.prune();
        Some(cache)
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{:016x}.json", fnv1a(key)))
    }

    pub fn get(&self, key: &str) -> Option<SearchResult> {
        let text = fs::read_to_string(self.path(key)).ok()?;
        let entry: Entry = serde_json::from_str(&text).ok()?;
        let age = Utc::now().timestamp() - entry.at;
        if entry.key != key || !(0..=self.ttl_secs).contains(&age) {
            return None;
        }
        let mut result = entry.result;
        result.cache_age_min = Some((age / 60) as u32);
        Some(result)
    }

    /// Best effort: a failed write only costs a future API call.
    pub fn put(&self, key: &str, result: &SearchResult) {
        let entry = Entry {
            at: Utc::now().timestamp(),
            key: key.to_string(),
            result: result.clone(),
        };
        if let Ok(text) = serde_json::to_string(&entry) {
            let _ = fs::write(self.path(key), text);
        }
    }

    /// Delete entries past their time-to-live so the folder cannot grow forever.
    fn prune(&self) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        let now = SystemTime::now();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let expired = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age.as_secs() as i64 > self.ttl_secs);
            if expired {
                let _ = fs::remove_file(path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a_matches_reference_vectors() {
        assert_eq!(fnv1a(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a("a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn round_trip_and_key_isolation() {
        let dir = std::env::temp_dir().join(format!("icarus-cache-test-{}", std::process::id()));
        let cache = Cache::open(dir.clone(), 6).expect("temp dir is writable");
        let result = SearchResult {
            itineraries: vec![],
            insights: None,
            browser_url: Some("u".into()),
            cache_age_min: None,
        };
        cache.put("LHR>NRT", &result);
        let hit = cache.get("LHR>NRT").expect("fresh entry is served");
        assert_eq!(hit.browser_url.as_deref(), Some("u"));
        assert_eq!(hit.cache_age_min, Some(0));
        assert!(cache.get("LHR>HND").is_none());
        let _ = fs::remove_dir_all(dir);
    }
}
