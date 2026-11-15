//! Compile-time escaped templates shared by standalone pages and modal fetches.
use crate::{
    discovery::{Resolution, Sort, State},
    snapshot::Snapshot,
};
use askama::Template;
use wrkgn_public_snapshot::{Detail, Discovery, Taxonomy};
include!(concat!(env!("OUT_DIR"), "/assets.rs"));
pub type RelatedProject = (i64, String, Option<String>);
pub fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
pub fn asset(path: &str) -> Option<(&'static str, &'static [u8])> {
    embedded_asset(path)
}
fn rendered(t: &impl Template) -> Result<String, String> {
    t.render().map_err(|e| e.to_string())
}
fn title(name: Option<&str>) -> String {
    name.map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Unnamed project")
        .into()
}
fn author(name: Option<&str>) -> String {
    name.map(|n| format!("@{n}"))
        .unwrap_or_else(|| "an unattributed HN account".into())
}
fn builders(names: &[String]) -> String {
    names
        .iter()
        .map(|n| format!("@{n}"))
        .collect::<Vec<_>>()
        .join(", ")
}
fn labels(slugs: &[String], taxonomy: &[Taxonomy]) -> Vec<String> {
    slugs
        .iter()
        .map(|slug| {
            taxonomy
                .iter()
                .find(|t| t.slug == *slug)
                .map(|t| t.label.clone())
                .unwrap_or_else(|| slug.clone())
        })
        .collect()
}
fn safe_url(value: &str) -> String {
    if wrkgn_public_snapshot::web_url(value) {
        value.into()
    } else {
        String::new()
    }
}
struct Date {
    iso: String,
    label: String,
}
fn date(seconds: i64) -> Date {
    match chrono::DateTime::from_timestamp(seconds, 0) {
        Some(d) => Date {
            iso: d.to_rfc3339(),
            label: d.format("%b %-d, %Y").to_string(),
        },
        None => Date {
            iso: String::new(),
            label: "Unknown date".into(),
        },
    }
}
struct Choice {
    slug: String,
    label: String,
    selected: bool,
}
struct Group {
    name: &'static str,
    label: &'static str,
    values: Vec<Choice>,
}
fn sorts(state: &State) -> Vec<Choice> {
    Sort::ALL
        .into_iter()
        .map(|s| Choice {
            slug: s.as_str().into(),
            label: s.label().into(),
            selected: s == state.sort,
        })
        .collect()
}
#[derive(Template)]
#[template(path = "controls.html")]
struct Controls {
    query: String,
    groups: Vec<Group>,
    sorts: Vec<Choice>,
    detail: bool,
}
fn controls(snapshot: &Snapshot, state: &State, detail: bool) -> Result<String, String> {
    let groups = [
        (
            "category",
            "Category",
            &snapshot.discovery.options.category,
            &state.category,
            &snapshot.metadata.categories,
        ),
        (
            "audience",
            "Audience",
            &snapshot.discovery.options.audience,
            &state.audience,
            &snapshot.metadata.audiences,
        ),
    ]
    .into_iter()
    .map(|(name, label, options, selected, taxonomy)| Group {
        name,
        label,
        values: options
            .iter()
            .map(|slug| Choice {
                slug: slug.clone(),
                label: taxonomy
                    .iter()
                    .find(|t| t.slug == *slug)
                    .map(|t| t.label.clone())
                    .unwrap_or_else(|| slug.clone()),
                selected: selected.contains(slug),
            })
            .collect(),
    })
    .collect();
    rendered(&Controls {
        query: state.q.clone(),
        groups,
        sorts: sorts(state),
        detail,
    })
}
#[derive(Template)]
#[template(path = "layout.html")]
struct Layout {
    title: String,
    description: String,
    canonical: String,
    noindex: bool,
    version: String,
    controls: String,
    content: String,
    style: &'static str,
    script: &'static str,
    icon: &'static str,
}
fn layout(
    snapshot: &Snapshot,
    state: &State,
    title: String,
    description: String,
    canonical: String,
    noindex: bool,
    content: String,
    detail: bool,
) -> Result<String, String> {
    rendered(&Layout {
        title,
        description,
        canonical,
        noindex,
        version: snapshot.metadata.version.clone(),
        controls: controls(snapshot, state, detail)?,
        content,
        style: STYLE_URL,
        script: SCRIPT_URL,
        icon: ICON_URL,
    })
}
#[derive(Template)]
#[template(path = "card.html")]
struct Card {
    id: i64,
    url: String,
    title: String,
    description: String,
    builders: String,
    date: Date,
    categories: Vec<String>,
}
fn card(p: &Discovery, snapshot: &Snapshot) -> Result<String, String> {
    rendered(&Card {
        id: p.id,
        url: format!("/projects/{}/", p.slug),
        title: title(p.name.as_deref()),
        description: p.description.clone(),
        builders: builders(&p.builder_usernames),
        date: date(p.latest_shared_at),
        categories: labels(&p.categories, &snapshot.metadata.categories),
    })
}
#[derive(Template)]
#[template(path = "listing.html")]
struct Listing {
    version: String,
    url: String,
    published: usize,
    count: String,
    total: usize,
    sorts: Vec<Choice>,
    cards: String,
    pages: usize,
    page: usize,
    previous: String,
    next: String,
}
pub fn listing(
    snapshot: &Snapshot,
    resolution: &Resolution,
    origin: &str,
) -> Result<String, String> {
    let state = &resolution.state;
    let page_url = |page| {
        let mut s = state.clone();
        s.page = page;
        s.url()
    };
    let cards = resolution
        .indices
        .iter()
        .map(|&i| card(&snapshot.discovery.projects[i], snapshot))
        .collect::<Result<Vec<_>, _>>()?
        .join("");
    let count = if resolution.total == 0 {
        "0 projects".into()
    } else {
        format!(
            "{}–{} of {} projects",
            (state.page - 1) * 30 + 1,
            ((state.page - 1) * 30 + resolution.indices.len()).min(resolution.total),
            resolution.total
        )
    };
    let content = rendered(&Listing {
        version: snapshot.metadata.version.clone(),
        url: state.url(),
        published: snapshot.discovery.projects.len(),
        count,
        total: resolution.total,
        sorts: sorts(state),
        cards,
        pages: resolution.pages,
        page: state.page,
        previous: if state.page > 1 {
            page_url(state.page - 1)
        } else {
            String::new()
        },
        next: if state.page < resolution.pages {
            page_url(state.page + 1)
        } else {
            String::new()
        },
    })?;
    let canonical = format!(
        "{origin}{}",
        if state.has_criteria() {
            "/".into()
        } else {
            state.url()
        }
    );
    layout(
        snapshot,
        state,
        "Explore projects".into(),
        "Explore projects and products shared on Hacker News.".into(),
        canonical,
        state.has_criteria(),
        content,
        false,
    )
}
struct Link {
    url: String,
    label: String,
}
struct Evidence {
    label: &'static str,
    author: String,
    date: Date,
    source: String,
    value: String,
    is_title: bool,
}
struct Mention {
    title: String,
    description: String,
    problem: String,
    audience: String,
    author: String,
    date: Date,
    source: String,
    excerpt: String,
    evidence: Vec<Evidence>,
    links: Vec<Link>,
}
#[derive(Template)]
#[template(path = "detail.html")]
struct Project {
    version: String,
    title: String,
    description: String,
    date: Date,
    audience: String,
    problem: String,
    mentions: Vec<Mention>,
    categories: Vec<String>,
    audiences: Vec<String>,
    builders: String,
    related: Vec<Link>,
}
pub fn project(
    snapshot: &Snapshot,
    detail: &Detail,
    related: &[RelatedProject],
    origin: &str,
) -> Result<String, String> {
    let p = snapshot
        .discovery
        .projects
        .iter()
        .find(|p| p.id == detail.id)
        .ok_or("detail not in discovery")?;
    let mentions = detail
        .mentions
        .iter()
        .map(|m| Mention {
            title: title(m.name.as_deref().or(detail.name.as_deref())),
            description: m.description.clone(),
            problem: m.problem.clone().unwrap_or_default(),
            audience: m.audience.clone().unwrap_or_default(),
            author: author(m.author.as_deref()),
            date: date(m.shared_at),
            source: safe_url(&m.source_url),
            excerpt: m.source_excerpt.clone(),
            evidence: m
                .evidence
                .iter()
                .map(|e| Evidence {
                    label: match e.source_field.as_str() {
                        "title" => "Story title",
                        "url" => "Submitted URL",
                        _ => "Source text",
                    },
                    author: author(e.author.as_deref()),
                    date: date(e.shared_at),
                    source: safe_url(&e.source_url),
                    value: e.value.clone(),
                    is_title: e.source_field == "title",
                })
                .collect(),
            links: m
                .links
                .iter()
                .filter_map(|v| {
                    let u = url::Url::parse(v).ok()?;
                    if !wrkgn_public_snapshot::web_url(v) {
                        return None;
                    }
                    Some(Link {
                        url: u.to_string(),
                        label: u.host_str().unwrap_or(v).into(),
                    })
                })
                .collect(),
        })
        .collect();
    let content = rendered(&Project {
        version: snapshot.metadata.version.clone(),
        title: title(p.name.as_deref()),
        description: p.description.clone(),
        date: date(p.latest_shared_at),
        audience: p.audience.clone().unwrap_or_default(),
        problem: p.problem.clone().unwrap_or_default(),
        mentions,
        categories: labels(&p.categories, &snapshot.metadata.categories),
        audiences: labels(&p.audience_labels, &snapshot.metadata.audiences),
        builders: builders(&p.builder_usernames),
        related: related
            .iter()
            .map(|(_, slug, name)| Link {
                url: format!("/projects/{slug}/"),
                label: title(name.as_deref()),
            })
            .collect(),
    })?;
    layout(
        snapshot,
        &State::default(),
        title(p.name.as_deref()),
        p.description.clone(),
        format!("{origin}/projects/{}/", p.slug),
        false,
        content,
        true,
    )
}
pub fn error(status: u16) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{status} · wrkgn</title><link rel=\"stylesheet\" href=\"{STYLE_URL}\"></head><body><main class=\"container\"><h1>{status}</h1><p>{}</p><a href=\"/\">Browse projects</a></main></body></html>",
        if status == 404 {
            "This project or page could not be found."
        } else {
            "This request could not be completed."
        }
    )
}
