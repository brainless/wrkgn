//! Compile-time escaped templates shared by standalone pages and modal fetches.
use crate::{
    discovery::{Facet, Resolution, Sort, State},
    metadata::{self, SITE_SUFFIX},
    snapshot::Snapshot,
};
use askama::Template;
use wrkgn_public_snapshot::{Detail, Discovery, Taxonomy};
include!(concat!(env!("OUT_DIR"), "/assets.rs"));
const LIST_DESCRIPTION: &str = "Explore projects and products shared on Hacker News.";
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
struct Tag {
    label: String,
    url: String,
}
/// Linked tags for a facet with landing pages.
fn tags(slugs: &[String], taxonomy: &[Taxonomy], facet: Facet) -> Vec<Tag> {
    slugs
        .iter()
        .zip(labels(slugs, taxonomy))
        .map(|(slug, label)| Tag {
            label,
            url: facet.landing_path(slug),
        })
        .collect()
}
fn taxonomy(snapshot: &Snapshot, facet: Facet) -> &[Taxonomy] {
    match facet {
        Facet::Category => &snapshot.metadata.categories,
        Facet::Audience => &snapshot.metadata.audiences,
    }
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
                // A landing page's fixed label shows as checked, so using a
                // control leads to the equivalent query result.
                selected: selected.contains(slug)
                    || state
                        .fixed
                        .as_ref()
                        .is_some_and(|f| f.facet.name() == name && f.slug == *slug),
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
const OG_IMAGE_ALT: &str = "wrkgn — Projects shared on Hacker News";
fn og_image(origin: &str) -> String {
    format!("{origin}{OG_IMAGE_URL}")
}
/// Per-page head metadata. `title` is the complete `<title>` text.
struct Head {
    title: String,
    /// Already truncated for meta/OG/Twitter use.
    description: String,
    canonical: String,
    noindex: bool,
    og_type: &'static str,
    /// Absolute URL of the preview image (`OG_IMAGE_URL` plus the public
    /// origin). `None` only for pages without social tags.
    image: Option<String>,
    /// Serialized JSON-LD with `<` escaped (see `metadata::json_ld`).
    json_ld: Option<String>,
}
#[derive(Template)]
#[template(path = "layout.html")]
struct Layout {
    title: String,
    description: String,
    canonical: String,
    noindex: bool,
    og_type: &'static str,
    twitter_card: &'static str,
    image: Option<String>,
    image_alt: &'static str,
    json_ld: Option<String>,
    version: String,
    controls: String,
    content: String,
    style: &'static str,
    script: &'static str,
    icon: &'static str,
    font_dm_sans: &'static str,
    font_source_serif_4: &'static str,
}
fn layout(
    snapshot: &Snapshot,
    state: &State,
    head: Head,
    content: String,
    detail: bool,
) -> Result<String, String> {
    rendered(&Layout {
        title: head.title,
        description: head.description,
        canonical: head.canonical,
        noindex: head.noindex,
        og_type: head.og_type,
        twitter_card: if head.image.is_some() {
            "summary_large_image"
        } else {
            "summary"
        },
        image: head.image,
        image_alt: OG_IMAGE_ALT,
        json_ld: head.json_ld,
        version: snapshot.metadata.version.clone(),
        controls: controls(snapshot, state, detail)?,
        content,
        style: STYLE_URL,
        script: SCRIPT_URL,
        icon: ICON_URL,
        font_dm_sans: FONT_DM_SANS_URL,
        font_source_serif_4: FONT_SOURCE_SERIF_4_URL,
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
    categories: Vec<Tag>,
}
fn card(p: &Discovery, snapshot: &Snapshot) -> Result<String, String> {
    rendered(&Card {
        id: p.id,
        url: format!("/projects/{}/", p.slug),
        title: title(p.name.as_deref()),
        description: p.description.clone(),
        builders: builders(&p.builder_usernames),
        date: date(p.latest_shared_at),
        categories: tags(
            &p.categories,
            &snapshot.metadata.categories,
            Facet::Category,
        ),
    })
}
#[derive(Template)]
#[template(path = "listing.html")]
struct Listing {
    eyebrow: String,
    heading: String,
    intro: String,
    summary: String,
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
struct Heading {
    eyebrow: String,
    title: String,
    intro: String,
    summary: String,
}
/// Results region shared by the main list and landing pages.
fn list_section(
    snapshot: &Snapshot,
    resolution: &Resolution,
    heading: Heading,
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
    rendered(&Listing {
        eyebrow: heading.eyebrow,
        heading: heading.title,
        intro: heading.intro,
        summary: heading.summary,
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
    })
}
pub fn listing(
    snapshot: &Snapshot,
    resolution: &Resolution,
    origin: &str,
) -> Result<String, String> {
    let state = &resolution.state;
    let content = list_section(
        snapshot,
        resolution,
        Heading {
            eyebrow: "Projects from HN posts".into(),
            title: "What are folks working on?".into(),
            intro: String::new(),
            summary: format!("{} projects published", snapshot.discovery.projects.len()),
        },
    )?;
    let canonical = format!(
        "{origin}{}",
        if state.has_criteria() {
            "/".into()
        } else {
            state.url()
        }
    );
    let is_home = !state.has_criteria() && state.page == 1;
    let (title, description) = if state.has_criteria() {
        let title = if state.q.is_empty() {
            "Filtered projects"
        } else {
            "Search results"
        };
        (format!("{title}{SITE_SUFFIX}"), LIST_DESCRIPTION.to_owned())
    } else if is_home {
        (
            format!("Projects shared on Hacker News{SITE_SUFFIX}"),
            LIST_DESCRIPTION.to_owned(),
        )
    } else {
        (
            format!(
                "Projects shared on Hacker News – Page {}{SITE_SUFFIX}",
                state.page
            ),
            format!("{LIST_DESCRIPTION} Page {}.", state.page),
        )
    };
    layout(
        snapshot,
        state,
        Head {
            title,
            description,
            canonical,
            noindex: state.has_criteria(),
            og_type: "website",
            image: Some(og_image(origin)),
            json_ld: is_home.then(|| metadata::website(origin)),
        },
        content,
        false,
    )
}
fn project_count(total: usize) -> String {
    format!(
        "{total} {}",
        if total == 1 { "project" } else { "projects" }
    )
}
/// Category or audience landing page. Its criterion is the
/// path, so it is indexable, self-canonical and independent of query noindex.
pub fn landing(
    snapshot: &Snapshot,
    resolution: &Resolution,
    origin: &str,
) -> Result<String, String> {
    let state = &resolution.state;
    let fixed = state.fixed.as_ref().ok_or("not a landing page")?;
    let entry = taxonomy(snapshot, fixed.facet)
        .iter()
        .find(|t| t.slug == fixed.slug);
    let label = entry.map_or(fixed.slug.as_str(), |t| t.label.as_str());
    let base = entry.map_or("", |t| t.description.trim());
    let base = if base.is_empty() {
        match fixed.facet {
            Facet::Category => format!("Projects shared on Hacker News in {label}."),
            Facet::Audience => format!("Projects shared on Hacker News for {label}."),
        }
    } else {
        base.to_owned()
    };
    let content = list_section(
        snapshot,
        resolution,
        Heading {
            eyebrow: fixed.facet.noun().into(),
            title: label.into(),
            intro: base.clone(),
            summary: project_count(resolution.total),
        },
    )?;
    let page_suffix = (state.page > 1).then(|| format!(" Page {}.", state.page));
    let description = match &page_suffix {
        Some(suffix) => format!(
            "{}{suffix}",
            metadata::truncate_words(&base, metadata::DESCRIPTION_BUDGET - suffix.chars().count())
        ),
        None => metadata::meta_description(&base),
    };
    let name = fixed.facet.page_name(label);
    let title = if state.page > 1 {
        format!("{name} – Page {}{SITE_SUFFIX}", state.page)
    } else {
        format!("{name}{SITE_SUFFIX}")
    };
    let canonical = format!("{origin}{}", state.url());
    layout(
        snapshot,
        state,
        Head {
            json_ld: Some(metadata::collection_page(&name, &description, &canonical)),
            title,
            description,
            canonical,
            noindex: false,
            og_type: "website",
            image: Some(og_image(origin)),
        },
        content,
        false,
    )
}
struct IndexEntry {
    url: String,
    label: String,
    description: String,
    count: String,
}
#[derive(Template)]
#[template(path = "landing_index.html")]
struct LandingIndex {
    heading: String,
    intro: String,
    summary: String,
    entries: Vec<IndexEntry>,
}
/// `/categories/`: every label with its description and project count.
pub fn landing_index(snapshot: &Snapshot, facet: Facet, origin: &str) -> Result<String, String> {
    let known = snapshot.discovery.options.get(facet);
    let mut entries: Vec<IndexEntry> = known
        .iter()
        .map(|slug| {
            let t = taxonomy(snapshot, facet).iter().find(|t| t.slug == *slug);
            IndexEntry {
                url: facet.landing_path(slug),
                label: t.map_or(slug.clone(), |t| t.label.clone()),
                description: t.map(|t| t.description.clone()).unwrap_or_default(),
                count: project_count(snapshot.discovery.count(facet, slug)),
            }
        })
        .collect();
    entries.sort_by(|a, b| a.label.cmp(&b.label).then(a.url.cmp(&b.url)));
    let heading = facet.plural_noun();
    let description = format!(
        "Browse projects shared on Hacker News by {}.",
        facet.noun().to_lowercase()
    );
    let content = rendered(&LandingIndex {
        heading: heading.into(),
        intro: description.clone(),
        summary: format!("{} {}", entries.len(), heading.to_lowercase()),
        entries,
    })?;
    let canonical = format!("{origin}{}", facet.index_path());
    layout(
        snapshot,
        &State::default(),
        Head {
            title: format!("{heading}{SITE_SUFFIX}"),
            json_ld: Some(metadata::collection_page(heading, &description, &canonical)),
            description,
            canonical,
            noindex: false,
            og_type: "website",
            image: Some(og_image(origin)),
        },
        content,
        true,
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
    categories: Vec<Tag>,
    audiences: Vec<Tag>,
    builders: String,
    related: Vec<Link>,
    similar_projects: Vec<Link>,
}
pub fn project(
    snapshot: &Snapshot,
    detail: &Detail,
    related: &[RelatedProject],
    origin: &str,
) -> Result<String, String> {
    let index = snapshot
        .discovery
        .projects
        .iter()
        .position(|p| p.id == detail.id)
        .ok_or("detail not in discovery")?;
    let p = &snapshot.discovery.projects[index];
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
        categories: tags(
            &p.categories,
            &snapshot.metadata.categories,
            Facet::Category,
        ),
        audiences: tags(
            &p.audience_labels,
            &snapshot.metadata.audiences,
            Facet::Audience,
        ),
        builders: builders(&p.builder_usernames),
        related: related
            .iter()
            .map(|(_, slug, name)| Link {
                url: format!("/projects/{slug}/"),
                label: title(name.as_deref()),
            })
            .collect(),
        similar_projects: snapshot
            .similar_projects
            .get(index)
            .iter()
            .map(|&i| {
                let s = &snapshot.discovery.projects[i as usize];
                Link {
                    url: format!("/projects/{}/", s.slug),
                    label: title(s.name.as_deref()),
                }
            })
            .collect(),
    })?;
    let canonical = format!("{origin}/projects/{}/", p.slug);
    let name = title(p.name.as_deref());
    let named = p.name.as_deref().is_some_and(|n| !n.trim().is_empty());
    let first_shared = detail.mentions.iter().map(|m| m.shared_at).min();
    layout(
        snapshot,
        &State::default(),
        Head {
            title: metadata::project_title(&name, &p.description, named),
            description: metadata::meta_description(&p.description),
            json_ld: Some(metadata::project_json_ld(
                &name,
                &p.description,
                &canonical,
                &p.builder_usernames,
                first_shared,
            )),
            canonical,
            noindex: false,
            og_type: "article",
            image: Some(og_image(origin)),
        },
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
