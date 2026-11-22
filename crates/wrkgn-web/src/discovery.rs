//! In-memory discovery and canonical URLs shared by ordinary and enhanced navigation.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
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

/// A taxonomy dimension. Query filters and landing pages share it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Facet {
    Category,
    Audience,
}
impl Facet {
    /// Facets that have landing pages (`/categories/…`, `/audiences/…`).
    pub const LANDING: [Self; 2] = [Self::Category, Self::Audience];
    /// Query parameter and form field name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Category => "category",
            Self::Audience => "audience",
        }
    }
    pub fn noun(self) -> &'static str {
        match self {
            Self::Category => "Category",
            Self::Audience => "Audience",
        }
    }
    pub fn plural_noun(self) -> &'static str {
        match self {
            Self::Category => "Categories",
            Self::Audience => "Audiences",
        }
    }
    /// Index page path, which is also the landing-page prefix.
    pub fn index_path(self) -> &'static str {
        match self {
            Self::Category => "/categories/",
            Self::Audience => "/audiences/",
        }
    }
    /// Page name for a landing page: "Games projects" or "Projects for Educators".
    pub fn page_name(self, label: &str) -> String {
        match self {
            Self::Category => format!("{label} projects"),
            Self::Audience => format!("Projects for {label}"),
        }
    }
    pub fn landing_path(self, slug: &str) -> String {
        format!("{}{slug}/", self.index_path())
    }
    fn labels(self, project: &Discovery) -> &[String] {
        match self {
            Self::Category => &project.categories,
            Self::Audience => &project.audience_labels,
        }
    }
}
/// A criterion fixed by the URL path (a landing page), unlike query filters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fixed {
    pub facet: Facet,
    pub slug: String,
}
impl Fixed {
    pub fn path(&self) -> String {
        self.facet.landing_path(&self.slug)
    }
}
/// The index page (`/categories/`) a path names, if any.
pub fn index_facet(path: &str) -> Option<Facet> {
    Facet::LANDING.into_iter().find(|f| f.index_path() == path)
}
/// `/categories/<slug>/` or `/categories/<slug>/page/<N>/`. The page segment is
/// returned raw; slugs are validated against the snapshot, not here.
pub fn parse_landing(path: &str) -> Option<(Fixed, Option<&str>)> {
    let facet = Facet::LANDING
        .into_iter()
        .find(|f| path.starts_with(f.index_path()))?;
    let (slug, tail) = path.strip_prefix(facet.index_path())?.split_once('/')?;
    if slug.is_empty() {
        return None;
    }
    let fixed = Fixed {
        facet,
        slug: slug.into(),
    };
    if tail.is_empty() {
        return Some((fixed, None));
    }
    let page = tail.strip_prefix("page/")?.strip_suffix('/')?;
    (!page.is_empty() && !page.contains('/')).then_some((fixed, Some(page)))
}
/// Any path served by the list or landing/index renderers.
pub fn is_discovery_path(path: &str) -> bool {
    path == "/"
        || path == "/projects/"
        || path
            .strip_prefix("/projects/page/")
            .and_then(|s| s.strip_suffix('/'))
            .is_some_and(|s| !s.is_empty() && !s.contains('/'))
        || index_facet(path).is_some()
        || parse_landing(path).is_some()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub q: String,
    pub category: Vec<String>,
    pub audience: Vec<String>,
    pub sort: Sort,
    pub page: usize,
    /// Landing-page criterion from the path. It is not a query criterion, so it
    /// never counts in `has_criteria` and the URL stays under its own path.
    pub fixed: Option<Fixed>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            q: String::new(),
            category: vec![],
            audience: vec![],
            sort: Sort::default(),
            page: 1,
            fixed: None,
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
    pub fn get(&self, facet: Facet) -> &BTreeSet<String> {
        match facet {
            Facet::Category => &self.category,
            Facet::Audience => &self.audience,
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
fn digits_page(segment: &str) -> usize {
    Some(segment)
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .map(page_number)
        .unwrap_or(1)
}
fn path_page(path: &str) -> usize {
    path.strip_prefix("/projects/page/")
        .and_then(|s| s.strip_suffix('/'))
        .map(digits_page)
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
    // A landing page is a fixed criterion: query filters, search and sort are
    // not read, so they redirect away; only `page` is honoured.
    if let Some((fixed, segment)) = parse_landing(path) {
        return State {
            page: first("page")
                .map(page_number)
                .unwrap_or_else(|| segment.map_or(1, digits_page)),
            fixed: Some(fixed),
            ..State::default()
        };
    }
    State {
        q: normalize_query(first("q").unwrap_or_default()),
        category: values("category"),
        audience: values("audience"),
        sort: Sort::parse(first("sort").unwrap_or_default()),
        page: first("page")
            .map(page_number)
            .unwrap_or_else(|| path_page(path)),
        fixed: None,
    }
}
/// URLSearchParams-compatible serialization (spaces use +, Unicode is UTF-8).
pub fn state_url(state: &State) -> String {
    if let Some(fixed) = &state.fixed {
        return if state.page > 1 {
            format!("{}page/{}/", fixed.path(), state.page)
        } else {
            fixed.path()
        };
    }
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
    /// A landing page whose label is unknown or has no projects.
    pub not_found: bool,
    /// Indices into DiscoveryIndex::projects, limited to the requested page.
    pub indices: Vec<usize>,
}

/// Discovery records hold only public card/search fields. Four sort orders are
/// prepared once on snapshot load; requests scan one and allocate at most 30 IDs.
pub struct DiscoveryIndex {
    pub projects: Vec<Discovery>,
    pub options: Options,
    counts: [BTreeMap<String, usize>; 2],
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
        let counts = [Facet::Category, Facet::Audience].map(|facet| {
            let mut counts = BTreeMap::new();
            for project in &projects {
                for slug in facet.labels(project) {
                    *counts.entry(slug.clone()).or_insert(0) += 1;
                }
            }
            counts
        });
        Self {
            projects,
            options,
            counts,
            orders,
        }
    }
    /// Projects carrying a label; computed once on load.
    pub fn count(&self, facet: Facet, slug: &str) -> usize {
        self.counts[facet as usize].get(slug).copied().unwrap_or(0)
    }
    pub fn page(&self, state: &State) -> Resolution {
        let query = state.q.to_lowercase();
        let mut total = 0usize;
        let offset = state.page.saturating_sub(1).saturating_mul(PAGE_SIZE);
        let mut indices = Vec::with_capacity(PAGE_SIZE);
        for &index in &self.orders[state.sort.slot()] {
            let p = &self.projects[index];
            if state
                .fixed
                .as_ref()
                .is_some_and(|f| !f.facet.labels(p).contains(&f.slug))
                || (!query.is_empty() && !p.searchable_text.contains(&query))
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
            not_found: false,
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
                not_found: false,
                total: 0,
                pages: 1,
                indices: vec![],
            };
        }
        if let Some(fixed) = &state.fixed
            && !self.options.get(fixed.facet).contains(&fixed.slug)
        {
            return Resolution {
                state,
                redirect: None,
                not_found: true,
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
