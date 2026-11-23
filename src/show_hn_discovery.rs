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

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant).
const fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// One UTC calendar day, the unit of Show HN coverage.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct UtcDay(i32);

impl UtcDay {
    /// The first day of backfill: the month Show HN stories begin to appear.
    pub const FLOOR: UtcDay = UtcDay(days_from_civil(2007, 2, 1) as i32);

    /// Strict `YYYY-MM-DD` (year 0001 to 9999) naming a real date.
    pub fn parse(value: &str) -> Result<Self, String> {
        let invalid = || format!("invalid date {value:?}; expected YYYY-MM-DD");
        let bytes = value.as_bytes();
        if !value.is_ascii()
            || bytes.len() != 10
            || bytes[4] != b'-'
            || bytes[7] != b'-'
            || !bytes
                .iter()
                .enumerate()
                .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
            || &value[..4] == "0000"
        {
            return Err(invalid());
        }
        let number =
            |range: std::ops::Range<usize>| value[range].parse::<i64>().map_err(|_| invalid());
        let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
        let days = days_from_civil(year, month, day);
        // Round-trip rejects impossible dates such as 2023-02-29.
        if civil_from_days(days) != (year, month, day) {
            return Err(invalid());
        }
        i32::try_from(days).map(Self).map_err(|_| invalid())
    }

    /// The UTC day containing the Unix time.
    pub fn at(unix: i64) -> Option<Self> {
        i32::try_from(unix.div_euclid(86_400)).ok().map(Self)
    }

    pub fn previous(self) -> Option<Self> {
        self.0.checked_sub(1).map(Self)
    }

    pub fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }

    /// The half-open timestamp range of the day.
    pub fn range(self) -> DateRange {
        let start = i64::from(self.0) * 86_400;
        DateRange {
            start,
            end: start + 86_400,
        }
    }
}

impl std::fmt::Display for UtcDay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (year, month, day) = civil_from_days(i64::from(self.0));
        write!(f, "{year:04}-{month:02}-{day:02}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateRange {
    pub start: i64,
    pub end: i64,
}
impl DateRange {
    /// Inclusive calendar endpoints become a half-open UTC timestamp range.
    pub fn parse(from: &str, through: &str) -> Result<Self, String> {
        let start = UtcDay::parse(from)?.range().start;
        let last = UtcDay::parse(through)?.range().end;
        if start >= last {
            return Err("--from must be the same date as --through or earlier".into());
        }
        Ok(Self { start, end: last })
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

pub struct HttpDiscovery {
    agent: ureq::Agent,
    control: Option<std::sync::Arc<crate::run::RunControl>>,
}
impl Default for HttpDiscovery {
    fn default() -> Self {
        Self {
            agent: ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(30)))
                .build()
                .new_agent(),
            control: None,
        }
    }
}
impl HttpDiscovery {
    /// Counts Algolia calls against the shared circuit breaker.
    pub fn with_control(mut self, control: std::sync::Arc<crate::run::RunControl>) -> Self {
        self.control = Some(control);
        self
    }

    fn count(&self, success: bool) {
        if let Some(control) = &self.control {
            if success {
                control.breaker.record_success(crate::run::Api::Algolia);
            } else {
                control.breaker.record_failure(crate::run::Api::Algolia);
            }
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
                    self.count(true);
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
                        // Only an exhausted retryable error counts as a failure.
                        if retry {
                            self.count(false);
                        }
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
    fn utc_day_parses_formats_and_steps() {
        let day = |s: &str| UtcDay::parse(s).unwrap();
        for value in [
            "2026-11-23",
            "2024-02-29",
            "2007-02-01",
            "0001-01-01",
            "9999-12-31",
        ] {
            assert_eq!(day(value).to_string(), value);
        }
        for invalid in [
            "2023-02-29",
            "2026-13-01",
            "2026-00-10",
            "2026-11-00",
            "2026-11-31",
            "2026-1-01",
            "0000-01-01",
            "20261123",
            "2026-11-2x",
            "",
            "２０２６-11-23",
        ] {
            assert!(UtcDay::parse(invalid).is_err(), "{invalid}");
        }
        assert_eq!(day("2026-03-01").previous(), Some(day("2026-02-28")));
        assert_eq!(day("2024-03-01").previous(), Some(day("2024-02-29")));
        assert_eq!(day("2026-01-01").previous(), Some(day("2025-12-31")));
        assert_eq!(day("2026-12-31").next(), Some(day("2027-01-01")));
        assert_eq!(UtcDay::at(0), Some(day("1970-01-01")));
        assert_eq!(UtcDay::at(-1), Some(day("1969-12-31")));
        assert_eq!(UtcDay::at(1_794_700_800), Some(day("2026-11-15")));
        assert_eq!(UtcDay::at(1_794_700_800 + 86_399), Some(day("2026-11-15")));
        assert_eq!(UtcDay::FLOOR, day("2007-02-01"));
        assert!(day("2007-01-31") < UtcDay::FLOOR);
        let range = day("2026-11-15").range();
        assert_eq!(range.start, 1_794_700_800);
        assert_eq!(range.end - range.start, 86_400);
    }
}
