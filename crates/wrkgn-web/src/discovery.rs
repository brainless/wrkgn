//! In-memory discovery and canonical URLs shared by ordinary and enhanced navigation.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use url::form_urlencoded;
use wrkgn_public_snapshot::Discovery;

pub const PAGE_SIZE: usize = 30;
pub const MAX_QUERY_CHARS: usize = 100;
const MAX_SAFE_INTEGER: usize = 9_007_199_254_740_991;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Sort {
    CommentsAsc,
    CommentsDesc,
    #[default]
    DateDesc,
    DateAsc,
}
impl Sort {
    pub const ALL: [Self; 4] = [
        Self::CommentsAsc,
        Self::CommentsDesc,
        Self::DateDesc,
        Self::DateAsc,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CommentsAsc => "comments-asc",
            Self::CommentsDesc => "comments-desc",
            Self::DateDesc => "date-desc",
            Self::DateAsc => "date-asc",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::CommentsAsc => "By comments (low to high)",
            Self::CommentsDesc => "By comments (high to low)",
            Self::DateDesc => "By date (recent first)",
            Self::DateAsc => "By date (oldest first)",
        }
    }
    fn parse(value: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|s| s.as_str() == value)
            .unwrap_or_default()
    }
    fn slot(self) -> usize {
        match self {
            Self::CommentsAsc => 0,
            Self::CommentsDesc => 1,
            Self::DateDesc => 2,
            Self::DateAsc => 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub q: String,
    pub category: Vec<String>,
    pub audience: Vec<String>,
    pub sort: Sort,
    pub page: usize,
}
impl Default for State {
    fn default() -> Self {
        Self {
            q: String::new(),
            category: vec![],
            audience: vec![],
            sort: Sort::default(),
            page: 1,
        }
    }
}
impl State {
    pub fn has_criteria(&self) -> bool {
        !self.q.is_empty()
            || !self.category.is_empty()
            || !self.audience.is_empty()
            || self.sort != Sort::default()
    }
    pub fn url(&self) -> String {
        state_url(self)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Options {
    pub category: BTreeSet<String>,
    pub audience: BTreeSet<String>,
}
impl Options {
    pub fn from_projects(projects: &[Discovery]) -> Self {
        Self {
            category: projects
                .iter()
                .flat_map(|p| p.categories.iter().cloned())
                .collect(),
            audience: projects
                .iter()
                .flat_map(|p| p.audience_labels.iter().cloned())
                .collect(),
        }
    }
    pub fn retain_known(&self, state: &mut State) {
        state.category.retain(|s| self.category.contains(s));
        state.audience.retain(|s| self.audience.contains(s));
    }
}

// JS trim includes FEFF and excludes U+0085, unlike Rust's is_whitespace.
fn js_space(c: char) -> bool {
    matches!(c, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}
pub fn normalize_query(q: &str) -> String {
    q.trim_matches(js_space)
        .chars()
        .take(MAX_QUERY_CHARS)
        .collect::<String>()
        .trim_end_matches(js_space)
        .to_owned()
}
fn page_number(raw: &str) -> usize {
    // Preserve Number() syntax accepted by the existing browser implementation,
    // then always emit a plain positive safe integer in the canonical URL.
    let raw = raw.trim_matches(js_space);
    let number = if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok().map(|v| v as f64)
    } else if let Some(binary) = raw.strip_prefix("0b").or_else(|| raw.strip_prefix("0B")) {
        u64::from_str_radix(binary, 2).ok().map(|v| v as f64)
    } else if let Some(octal) = raw.strip_prefix("0o").or_else(|| raw.strip_prefix("0O")) {
        u64::from_str_radix(octal, 8).ok().map(|v| v as f64)
    } else {
        raw.parse::<f64>().ok()
    };
    number
        .filter(|n| n.is_finite() && *n > 0.0 && n.fract() == 0.0 && *n <= MAX_SAFE_INTEGER as f64)
        .map(|n| n as usize)
        .unwrap_or(1)
}
fn path_page(path: &str) -> usize {
    path.strip_prefix("/projects/page/")
        .and_then(|s| s.strip_suffix('/'))
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .map(page_number)
        .unwrap_or(1)
}
/// Normalize syntax only. Unknown filter values remain until the snapshot phase.
pub fn read_state(path: &str, query: Option<&str>) -> State {
    let pairs: Vec<_> = form_urlencoded::parse(query.unwrap_or_default().as_bytes()).collect();
    let first = |key| {
        pairs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_ref())
    };
    let values = |key| {
        pairs
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.to_string())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    };
    State {
        q: normalize_query(first("q").unwrap_or_default()),
        category: values("category"),
        audience: values("audience"),
        sort: Sort::parse(first("sort").unwrap_or_default()),
        page: first("page")
            .map(page_number)
            .unwrap_or_else(|| path_page(path)),
    }
}
/// URLSearchParams-compatible serialization (spaces use +, Unicode is UTF-8).
pub fn state_url(state: &State) -> String {
    let mut params = form_urlencoded::Serializer::new(String::new());
    if !state.q.is_empty() {
        params.append_pair("q", &state.q);
    }
    for value in &state.category {
        params.append_pair("category", value);
    }
    for value in &state.audience {
        params.append_pair("audience", value);
    }
    if state.sort != Sort::default() {
        params.append_pair("sort", state.sort.as_str());
    }
    if !state.has_criteria() {
        return if state.page > 1 {
            format!("/projects/page/{}/", state.page)
        } else {
            "/".into()
        };
    }
    if state.page > 1 {
        params.append_pair("page", &state.page.to_string());
    }
    format!("/?{}", params.finish())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirect {
    pub status: u16,
    pub location: String,
}
#[derive(Debug, Clone)]
pub struct Resolution {
    pub state: State,
    pub redirect: Option<Redirect>,
    pub total: usize,
    pub pages: usize,
    /// Indices into DiscoveryIndex::projects, limited to the requested page.
    pub indices: Vec<usize>,
}

/// Discovery records hold only public card/search fields. Four sort orders are
/// prepared once on snapshot load; requests scan one and allocate at most 30 IDs.
pub struct DiscoveryIndex {
    pub projects: Vec<Discovery>,
    pub options: Options,
    orders: [Vec<usize>; 4],
}
impl DiscoveryIndex {
    pub fn new(mut projects: Vec<Discovery>) -> Self {
        for project in &mut projects {
            project.searchable_text = project.searchable_text.to_lowercase();
        }
        let options = Options::from_projects(&projects);
        let orders = std::array::from_fn(|slot| {
            let sort = Sort::ALL[slot];
            let mut order: Vec<_> = (0..projects.len()).collect();
            order.sort_unstable_by(|&a, &b| {
                let a = &projects[a];
                let b = &projects[b];
                let date = b.latest_shared_at.cmp(&a.latest_shared_at);
                match sort {
                    Sort::DateDesc => date,
                    Sort::DateAsc => date.reverse(),
                    Sort::CommentsAsc => a.comment_count.cmp(&b.comment_count).then(date),
                    Sort::CommentsDesc => b.comment_count.cmp(&a.comment_count).then(date),
                }
                .then_with(|| a.id.cmp(&b.id))
            });
            order
        });
        Self {
            projects,
            options,
            orders,
        }
    }
    pub fn page(&self, state: &State) -> Resolution {
        let query = state.q.to_lowercase();
        let mut total = 0usize;
        let offset = state.page.saturating_sub(1).saturating_mul(PAGE_SIZE);
        let mut indices = Vec::with_capacity(PAGE_SIZE);
        for &index in &self.orders[state.sort.slot()] {
            let p = &self.projects[index];
            if (!query.is_empty() && !p.searchable_text.contains(&query))
                || (!state.category.is_empty()
                    && !state.category.iter().any(|s| p.categories.contains(s)))
                || (!state.audience.is_empty()
                    && !state.audience.iter().any(|s| p.audience_labels.contains(s)))
            {
                continue;
            }
            if total >= offset && indices.len() < PAGE_SIZE {
                indices.push(index);
            }
            total += 1;
        }
        Resolution {
            state: state.clone(),
            redirect: None,
            total,
            pages: total.div_ceil(PAGE_SIZE).max(1),
            indices,
        }
    }
    /// Syntax normalization takes precedence, avoiding permanent redirects whose
    /// target changes when a snapshot adds an option or another results page.
    pub fn resolve(&self, path: &str, query: Option<&str>) -> Resolution {
        let raw = match query {
            Some(q) => format!("{path}?{q}"),
            None => path.to_owned(),
        };
        let mut state = read_state(path, query);
        let syntax_url = state.url();
        if raw != syntax_url {
            return Resolution {
                state,
                redirect: Some(Redirect {
                    status: 301,
                    location: syntax_url,
                }),
                total: 0,
                pages: 1,
                indices: vec![],
            };
        }
        let before = state.url();
        self.options.retain_known(&mut state);
        let mut page = self.page(&state);
        if state.page > page.pages {
            state.page = page.pages;
        }
        let after = state.url();
        if before != after {
            page.state = state;
            page.redirect = Some(Redirect {
                status: 302,
                location: after,
            });
            page.indices.clear();
        }
        page
    }
    /// Order for related-project links and sitemap pagination.
    pub fn newest(&self) -> impl Iterator<Item = &Discovery> {
        self.orders[Sort::DateDesc.slot()]
            .iter()
            .map(|&i| &self.projects[i])
    }
}
