//! Deterministic Show HN selection and bounded Algolia discovery.
use serde::Deserialize;
use std::collections::BTreeMap;
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
pub trait Discovery {
    fn stories(&self, range: DateRange) -> Result<Vec<Candidate>, String>;
}

#[derive(Deserialize)]
pub struct SearchHit {
    #[serde(rename = "objectID")]
    pub id: String,
    pub title: Option<String>,
    pub created_at_i: Option<i64>,
}
#[derive(Deserialize)]
pub struct SearchPage {
    pub hits: Vec<SearchHit>,
    #[serde(rename = "nbPages")]
    pub pages: usize,
    #[serde(rename = "nbHits")]
    pub total: usize,
    #[serde(rename = "hitsPerPage")]
    pub per_page: usize,
    #[serde(rename = "exhaustiveNbHits", default)]
    pub exhaustive: bool,
}

/// Split full search windows before consuming pages; all windows are disjoint.
/// Estimated counts alone do not imply missing pages.
pub fn discover(
    range: DateRange,
    mut fetch: impl FnMut(DateRange, usize) -> Result<SearchPage, String>,
) -> Result<Vec<Candidate>, String> {
    if range.start >= range.end {
        return Err("empty discovery window".into());
    }
    let mut pending = vec![range];
    let mut candidates = BTreeMap::new();
    while let Some(window) = pending.pop() {
        let first = fetch(window, 0)?;
        if first.per_page == 0 {
            return Err("discovery returned zero page size".into());
        }
        let capacity = first.pages.saturating_mul(first.per_page);
        if capacity >= 1000 || (first.exhaustive && first.total > capacity) {
            if window.end - window.start <= 1 {
                return Err(format!(
                    "Show HN search exceeds pagination limit in one-second window {}",
                    window.start
                ));
            }
            let midpoint = window.start + (window.end - window.start) / 2;
            pending.push(DateRange {
                start: window.start,
                end: midpoint,
            });
            pending.push(DateRange {
                start: midpoint,
                end: window.end,
            });
            continue;
        }
        let pages = first.pages;
        let mut current = first;
        for page in 0..pages.max(1) {
            if page > 0 {
                current = fetch(window, page)?;
            }
            if (pages > 0 || current.total > 0) && current.hits.is_empty() {
                return Err("Show HN discovery returned an incomplete page".into());
            }
            if current.pages != pages {
                return Err("Show HN search pagination changed; rerun discovery".into());
            }
            for hit in current.hits.drain(..) {
                if !hit.title.as_deref().is_some_and(matches_title) {
                    continue;
                }
                let id = hit
                    .id
                    .parse::<i64>()
                    .ok()
                    .filter(|id| *id > 0)
                    .ok_or("invalid Show HN candidate ID")?;
                let timestamp = hit.created_at_i.ok_or("Show HN candidate has no date")?;
                if window.contains(timestamp) {
                    candidates.insert(id, timestamp);
                }
            }
        }
    }
    let mut stories: Vec<_> = candidates
        .into_iter()
        .map(|(id, timestamp)| Candidate { id, timestamp })
        .collect();
    stories.sort_by_key(|s| (std::cmp::Reverse(s.timestamp), std::cmp::Reverse(s.id)));
    Ok(stories)
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
    fn stories(&self, range: DateRange) -> Result<Vec<Candidate>, String> {
        discover(range, |window, page| self.page(window, page))
    }
}
impl HttpDiscovery {
    fn page(&self, window: DateRange, page: usize) -> Result<SearchPage, String> {
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
                &format!("created_at_i>={},created_at_i<{}", window.start, window.end),
            )
            .append_pair("hitsPerPage", "100")
            .append_pair("page", &page.to_string());
        for attempt in 0..3 {
            match self.agent.get(url.as_str()).call() {
                Ok(mut response) => {
                    return response
                        .body_mut()
                        .read_json()
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
    fn page(hits: serde_json::Value, pages: usize, total: usize, exhaustive: bool) -> SearchPage {
        serde_json::from_value(serde_json::json!({"hits":hits,"nbPages":pages,"nbHits":total,"hitsPerPage":100,"exhaustiveNbHits":exhaustive})).unwrap()
    }
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
    #[test]
    fn pagination_deduplication_ordering_and_estimates() {
        let mut calls = vec![];
        let result = discover(DateRange { start: 0, end: 100 }, |_, p| {
            calls.push(p);
            Ok(page(
                serde_json::json!([
                    {"objectID":"10","title":"Show HN: A","created_at_i":50},
                    {"objectID":if p==0 {"20"} else {"30"},"title":"Show HN: B","created_at_i":60},
                    {"objectID":"99","title":"Ask HN: C","created_at_i":90},
                    {"objectID":"98","title":"Show HN: D","created_at_i":100}
                ]),
                2,
                999,
                false,
            ))
        })
        .unwrap();
        assert_eq!(calls, [0, 1]);
        assert_eq!(
            result.iter().map(|s| s.id).collect::<Vec<_>>(),
            [30, 20, 10]
        );
    }
    #[test]
    fn partitions_nonoverlapping_windows_and_fails_irreducible_limits() {
        let mut calls = vec![];
        let result=discover(DateRange{start:0,end:2},|w,p|{
            calls.push((w,p));
            if w.end-w.start>1 {Ok(page(serde_json::json!([]),10,1001,true))}
            else {Ok(page(serde_json::json!([{"objectID":(w.start+1).to_string(),"title":"Show HN: A","created_at_i":w.start}]),1,1,true))}
        }).unwrap();
        assert_eq!(result.iter().map(|s| s.id).collect::<Vec<_>>(), [2, 1]);
        assert_eq!(calls.len(), 3);
        assert!(
            discover(DateRange { start: 0, end: 1 }, |_, _| Ok(page(
                serde_json::json!([]),
                10,
                1001,
                true
            )))
            .unwrap_err()
            .contains("one-second")
        );
    }
    #[test]
    fn missing_page_and_transport_failure_are_explicit() {
        assert!(
            discover(DateRange { start: 0, end: 2 }, |_, _| Ok(page(
                serde_json::json!([]),
                1,
                2,
                false
            )))
            .is_err()
        );
        assert!(
            discover(DateRange { start: 0, end: 2 }, |_, _| Err(
                "offline failure".into()
            ))
            .unwrap_err()
            .contains("offline failure")
        );
        assert!(
            discover(DateRange { start: 0, end: 2 }, |_, _| Ok(page(
                serde_json::json!([]),
                0,
                0,
                false
            )))
            .unwrap()
            .is_empty()
        );
    }
}
