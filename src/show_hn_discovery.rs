//! Deterministic Show HN selection and bounded Algolia discovery.
use serde::Deserialize;
use std::collections::BTreeSet;
use std::time::Duration;

pub fn matches_title(title: &str) -> bool {
    title
        .trim()
        .get(..8)
        .is_some_and(|s| s.eq_ignore_ascii_case("show hn:"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateRange {
    pub start: i64,
    pub end: i64,
}
impl DateRange {
    /// Inclusive calendar endpoints become a half-open UTC timestamp range.
    pub fn parse(from: &str, through: &str) -> Result<Self, String> {
        fn timestamp(value: &str) -> Result<i64, String> {
            let invalid = || format!("invalid date {value:?}; expected YYYY-MM-DD");
            if value.len() != 10
                || !value.is_ascii()
                || value.as_bytes()[4] != b'-'
                || value.as_bytes()[7] != b'-'
                || !value
                    .bytes()
                    .enumerate()
                    .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
                || &value[..4] == "0000"
            {
                return Err(invalid());
            }
            let connection = rusqlite::Connection::open_in_memory().map_err(|e| e.to_string())?;
            let (normalized, stamp): (Option<String>, Option<i64>) = connection
                .query_row(
                    "SELECT strftime('%Y-%m-%d',julianday(?1)), unixepoch(?1)",
                    [value],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(|e| e.to_string())?;
            if normalized.as_deref() != Some(value) {
                return Err(invalid());
            }
            stamp.ok_or_else(invalid)
        }
        let start = timestamp(from)?;
        let last = timestamp(through)?;
        if start > last {
            return Err("--from must be the same date as --through or earlier".into());
        }
        Ok(Self {
            start,
            end: last.checked_add(86400).ok_or("date range overflow")?,
        })
    }
    pub fn contains(self, timestamp: i64) -> bool {
        self.start <= timestamp && timestamp < self.end
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub id: i64,
    pub timestamp: i64,
}
pub const DEFAULT_BATCH_SIZE: usize = 100;
/// Algolia refuses to return more than 1,000 hits for one request.
pub const MAX_BATCH_SIZE: usize = 1000;

#[derive(Deserialize)]
pub struct SearchHit {
    #[serde(rename = "objectID")]
    pub id: String,
    pub title: Option<String>,
    pub created_at_i: Option<i64>,
}
pub trait Discovery {
    /// One search request: up to `limit` Show HN title hits with
    /// `start <= created_at_i <= end` (both inclusive), newest first.
    fn search(&self, start: i64, end: i64, limit: usize) -> Result<Vec<SearchHit>, String>;
}

/// Walks a range backward with a timestamp cursor, one search per batch.
/// Page numbers shift as stories are posted and stop at 1,000 hits; a
/// timestamp does not. The cursor is inclusive so stories sharing the oldest
/// timestamp of a full page are not lost; IDs already returned at that
/// timestamp are skipped on the next request.
pub struct Batches<'a, D> {
    discovery: &'a D,
    range: DateRange,
    limit: usize,
    cursor: i64,
    handled: BTreeSet<i64>,
    done: bool,
}
impl<'a, D: Discovery> Batches<'a, D> {
    pub fn new(discovery: &'a D, range: DateRange, limit: usize) -> Result<Self, String> {
        if limit == 0 || limit > MAX_BATCH_SIZE {
            return Err(format!("batch size must be 1..={MAX_BATCH_SIZE}"));
        }
        Ok(Self {
            discovery,
            range,
            limit,
            cursor: range.end - 1,
            handled: BTreeSet::new(),
            done: range.start >= range.end,
        })
    }
    pub fn finished(&self) -> bool {
        self.done
    }
    /// The newest timestamp the next search may return.
    pub fn cursor(&self) -> i64 {
        self.cursor
    }
    /// Searches once. `None` means the range is exhausted without a new hit;
    /// otherwise stories are newest first (possibly none if titles mismatch).
    pub fn next_batch(&mut self) -> Result<Option<Vec<Candidate>>, String> {
        if self.done {
            return Ok(None);
        }
        let hits = self
            .discovery
            .search(self.range.start, self.cursor, self.limit)?;
        let raw = hits.len();
        if raw > self.limit {
            return Err("Show HN discovery returned more hits than requested".into());
        }
        let mut seen = Vec::new();
        for hit in hits {
            let matches = hit.title.as_deref().is_some_and(matches_title);
            let Some(timestamp) = hit.created_at_i else {
                if matches {
                    return Err("Show HN candidate has no date".into());
                }
                continue;
            };
            if timestamp < self.range.start || timestamp > self.cursor {
                continue;
            }
            let id = hit.id.parse::<i64>().ok().filter(|id| *id > 0);
            let id = match (id, matches) {
                (Some(id), _) => id,
                (None, true) => return Err("invalid Show HN candidate ID".into()),
                (None, false) => continue,
            };
            seen.push((id, timestamp, matches));
        }
        if raw == 0 {
            self.done = true;
            return Ok(None);
        }
        let fresh: Vec<_> = seen
            .iter()
            .filter(|(id, ts, _)| !(*ts == self.cursor && self.handled.contains(id)))
            .copied()
            .collect();
        let full = raw >= self.limit;
        if fresh.is_empty() && !full {
            self.done = true;
            return Ok(None);
        }
        if fresh.is_empty() && full {
            return Err(format!(
                "Show HN discovery cannot advance past timestamp {}: a full page of {} hits holds only stories already handled; rerun with a larger --batch-size",
                self.cursor, self.limit
            ));
        }
        if full {
            let oldest = seen
                .iter()
                .map(|(_, ts, _)| *ts)
                .min()
                .unwrap_or(self.cursor);
            if oldest != self.cursor {
                self.handled.clear();
            }
            self.handled.extend(
                seen.iter()
                    .filter(|(_, ts, _)| *ts == oldest)
                    .map(|(id, ..)| *id),
            );
            self.cursor = oldest;
        } else {
            self.done = true;
        }
        let mut stories: Vec<_> = fresh
            .into_iter()
            .filter(|(.., matches)| *matches)
            .map(|(id, timestamp, _)| Candidate { id, timestamp })
            .collect();
        stories.sort_by_key(|s| (std::cmp::Reverse(s.timestamp), std::cmp::Reverse(s.id)));
        stories.dedup_by_key(|s| s.id);
        Ok(Some(stories))
    }
}

/// Formats a Unix timestamp for progress output.
pub fn utc(timestamp: i64) -> String {
    rusqlite::Connection::open_in_memory()
        .and_then(|c| {
            c.query_row(
                "SELECT strftime('%Y-%m-%d %H:%M:%S',?1,'unixepoch')||' UTC'",
                [timestamp],
                |r| r.get(0),
            )
        })
        .unwrap_or_else(|_| timestamp.to_string())
}

pub struct HttpDiscovery {
    agent: ureq::Agent,
}
impl Default for HttpDiscovery {
    fn default() -> Self {
        Self {
            agent: ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(30)))
                .build()
                .new_agent(),
        }
    }
}
impl Discovery for HttpDiscovery {
    fn search(&self, start: i64, end: i64, limit: usize) -> Result<Vec<SearchHit>, String> {
        #[derive(Deserialize)]
        struct Page {
            hits: Vec<SearchHit>,
        }
        let mut url = url::Url::parse("https://hn.algolia.com/api/v1/search_by_date").unwrap();
        url.query_pairs_mut()
            .append_pair("query", "Show HN:")
            .append_pair("tags", "story")
            .append_pair("restrictSearchableAttributes", "title")
            .append_pair("typoTolerance", "false")
            .append_pair("queryType", "prefixNone")
            .append_pair("removeWordsIfNoResults", "none")
            .append_pair(
                "numericFilters",
                &format!("created_at_i>={start},created_at_i<={end}"),
            )
            .append_pair("hitsPerPage", &limit.to_string())
            .append_pair("page", "0");
        for attempt in 0..3 {
            match self.agent.get(url.as_str()).call() {
                Ok(mut response) => {
                    return response
                        .body_mut()
                        .read_json::<Page>()
                        .map(|page| page.hits)
                        .map_err(|e| format!("invalid Show HN discovery response: {e}"));
                }
                Err(error) => {
                    let retry = matches!(
                        error,
                        ureq::Error::StatusCode(429 | 500..=599)
                            | ureq::Error::Timeout(_)
                            | ureq::Error::Io(_)
                            | ureq::Error::HostNotFound
                    );
                    if !retry || attempt == 2 {
                        return Err(format!("Show HN discovery failed: {error}"));
                    }
                    std::thread::sleep(Duration::from_secs(1 << attempt));
                }
            }
        }
        unreachable!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn title_and_date_boundaries() {
        for title in ["Show HN: Alpha", " show hn: Title "] {
            assert!(matches_title(title));
        }
        for title in [
            "Ask HN: Show HN: Alpha",
            "Show HN Alpha",
            "Show HN-ish: Alpha",
            "",
        ] {
            assert!(!matches_title(title));
        }
        let range = DateRange::parse("2024-02-29", "2024-02-29").unwrap();
        assert_eq!(range.end - range.start, 86400);
        assert!(range.contains(range.start));
        assert!(!range.contains(range.end));
        for (a, b) in [
            ("2023-02-29", "2023-03-01"),
            ("2026-11-02", "2026-11-01"),
            ("2026-1-01", "2026-11-01"),
            ("0000-01-01", "2026-11-01"),
        ] {
            assert!(DateRange::parse(a, b).is_err());
        }
    }
    fn hit(id: i64, ts: i64) -> SearchHit {
        SearchHit {
            id: id.to_string(),
            title: Some("Show HN: A".into()),
            created_at_i: Some(ts),
        }
    }
    /// Serves a sorted corpus the way search_by_date does, recording requests.
    struct Corpus {
        hits: Vec<(i64, i64)>,
        calls: std::cell::RefCell<Vec<(i64, i64, usize)>>,
    }
    impl Discovery for Corpus {
        fn search(&self, start: i64, end: i64, limit: usize) -> Result<Vec<SearchHit>, String> {
            self.calls.borrow_mut().push((start, end, limit));
            let mut hits: Vec<_> = self
                .hits
                .iter()
                .filter(|(_, ts)| (start..=end).contains(ts))
                .collect();
            hits.sort_by_key(|(id, ts)| (std::cmp::Reverse(*ts), std::cmp::Reverse(*id)));
            Ok(hits
                .into_iter()
                .take(limit)
                .map(|(id, ts)| hit(*id, *ts))
                .collect())
        }
    }
    fn walk(corpus: &Corpus, limit: usize) -> Result<Vec<Vec<i64>>, String> {
        let mut batches = Batches::new(corpus, DateRange { start: 0, end: 100 }, limit)?;
        let mut all = vec![];
        while !batches.finished() {
            if let Some(batch) = batches.next_batch()? {
                all.push(batch.iter().map(|s| s.id).collect());
            }
        }
        Ok(all)
    }
    #[test]
    fn cursor_walks_newest_first_without_losing_ties() {
        // Two stories share timestamp 50 across a batch edge.
        let corpus = Corpus {
            hits: vec![(1, 90), (2, 80), (3, 50), (4, 50), (7, 10)],
            calls: Default::default(),
        };
        let batches = walk(&corpus, 3).unwrap();
        let flat: Vec<_> = batches.iter().flatten().copied().collect();
        assert_eq!(flat, [1, 2, 4, 3, 7]);
        assert_eq!(batches[0], [1, 2, 4]);
        assert_eq!(batches[1], [3, 7]);
        // The first request is bounded by the inclusive end of the range.
        assert_eq!(corpus.calls.borrow()[0], (0, 99, 3));
        assert_eq!(corpus.calls.borrow()[1].1, 50);
    }
    #[test]
    fn full_page_of_handled_ties_fails_instead_of_looping() {
        let corpus = Corpus {
            hits: vec![(1, 50), (2, 50), (3, 50)],
            calls: Default::default(),
        };
        assert!(walk(&corpus, 2).unwrap_err().contains("cannot advance"));
        // A page larger than the tie reaches the end of the range.
        assert_eq!(walk(&corpus, 4).unwrap(), [vec![3, 2, 1]]);
    }
    #[test]
    fn empty_range_and_bad_limits() {
        let corpus = Corpus {
            hits: vec![],
            calls: Default::default(),
        };
        assert!(walk(&corpus, 10).unwrap().is_empty());
        assert_eq!(corpus.calls.borrow().len(), 1);
        for limit in [0, MAX_BATCH_SIZE + 1] {
            assert!(walk(&corpus, limit).is_err());
        }
    }
    #[test]
    fn transport_and_malformed_hits_are_explicit() {
        struct Failing;
        impl Discovery for Failing {
            fn search(&self, _: i64, _: i64, _: usize) -> Result<Vec<SearchHit>, String> {
                Err("offline failure".into())
            }
        }
        let mut batches = Batches::new(&Failing, DateRange { start: 0, end: 2 }, 10).unwrap();
        assert!(batches.next_batch().unwrap_err().contains("offline"));
        struct Undated;
        impl Discovery for Undated {
            fn search(&self, _: i64, _: i64, _: usize) -> Result<Vec<SearchHit>, String> {
                Ok(vec![SearchHit {
                    id: "1".into(),
                    title: Some("Show HN: A".into()),
                    created_at_i: None,
                }])
            }
        }
        let mut batches = Batches::new(&Undated, DateRange { start: 0, end: 2 }, 10).unwrap();
        assert!(batches.next_batch().unwrap_err().contains("no date"));
    }
    #[test]
    fn utc_formats_timestamps() {
        assert_eq!(utc(0), "1970-01-01 00:00:00 UTC");
    }
}
