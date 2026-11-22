//! Category and audience landing pages and index over an offline fixture snapshot.
use axum::{
    body::Body,
    http::{Request, header},
};
use http_body_util::BodyExt;
use tower::ServiceExt;
use wrkgn_public_snapshot::{
    self as public, Detail, Discovery, Evidence, Mention, Metadata, Taxonomy,
};
use wrkgn_web::{
    config::Config,
    server::{AppState, router, sitemap},
};

const CATEGORIES: [&str; 15] = [
    "ai-automation",
    "developer-tools",
    "productivity",
    "education",
    "games",
    "creative-media",
    "data-analytics",
    "business-finance",
    "community-civic",
    "hardware-diy",
    "infrastructure-hosting",
    "security-privacy",
    "science-research",
    "health-fitness",
    "lifestyle-hobbies",
];
const AUDIENCES: [&str; 13] = [
    "developers",
    "system-administrators",
    "students-learners",
    "educators",
    "researchers",
    "creative-professionals",
    "gamers",
    "makers-hobbyists",
    "business-professionals",
    "founders-business-owners",
    "community-organizers",
    "personal-use",
    "general-public",
];
const HOSTILE: &str = "Small </script><script>alert(1)</script> \"q\" & <b>x</b>";

struct Fixture {
    dir: tempfile::TempDir,
    state: AppState,
}
fn tax(slug: &str, label: &str, description: &str) -> Taxonomy {
    Taxonomy {
        slug: slug.into(),
        label: label.into(),
        description: description.into(),
    }
}
fn project(id: i64, category: &str) -> Discovery {
    Discovery {
        id,
        slug: format!("p{id}"),
        name: Some(format!("Project {id}")),
        description: format!("Description {id}"),
        problem: None,
        audience: None,
        builder_usernames: vec![],
        categories: vec![category.into()],
        audience_labels: vec![if id <= 35 { "developers" } else { "educators" }.into()],
        latest_shared_at: 1_700_000_000 + id,
        comment_count: 0,
        searchable_text: format!("project {id}"),
    }
}
fn detail(p: &Discovery) -> Detail {
    Detail {
        id: p.id,
        slug: p.slug.clone(),
        name: p.name.clone(),
        mentions: vec![Mention {
            name: p.name.clone(),
            description: p.description.clone(),
            problem: None,
            audience: None,
            source_item_id: p.id,
            source_url: format!("https://news.ycombinator.com/item?id={}", p.id),
            author: Some("alice".into()),
            shared_at: p.latest_shared_at,
            source_excerpt: "excerpt".into(),
            builder_usernames: vec![],
            links: vec![],
            evidence: vec![Evidence {
                source_item_id: p.id,
                source_field: "body".into(),
                value: "excerpt".into(),
                author: Some("alice".into()),
                shared_at: p.latest_shared_at,
                source_url: format!("https://news.ycombinator.com/item?id={}", p.id),
            }],
        }],
    }
}
/// 35 projects in developer-tools (audience developers), 3 in games
/// (educators), none in health-fitness or the other audiences.
async fn fixture(render_limit: usize) -> Fixture {
    let mut projects: Vec<Discovery> = (1..=35).map(|i| project(i, "developer-tools")).collect();
    projects.extend((36..=38).map(|i| project(i, "games")));
    let details: Vec<Detail> = projects.iter().map(detail).collect();
    let metadata = Metadata {
        schema_version: 6,
        version: "0123456789abcdef".into(),
        exported_at: 100,
        categories: CATEGORIES
            .map(|slug| match slug {
                "developer-tools" => tax(slug, "Big Things", "Large projects of every kind."),
                "games" => tax(slug, "Small", HOSTILE),
                _ => tax(slug, slug, slug),
            })
            .into(),
        audiences: AUDIENCES
            .map(|slug| match slug {
                "developers" => tax(slug, "Coders", "People who write software."),
                "educators" => tax(slug, "Teachers", HOSTILE),
                _ => tax(slug, slug, slug),
            })
            .into(),
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("public.sqlite");
    public::write_database(&path, &metadata, &projects, &details).unwrap();
    let config = Config {
        snapshot: path,
        public_origin: "https://example.test".into(),
        render_limit,
        ..Config::default()
    };
    Fixture {
        dir,
        state: AppState::load(config).await.unwrap(),
    }
}
async fn get(app: &axum::Router, uri: &str) -> (u16, String, axum::http::HeaderMap) {
    let r = app
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let (status, headers) = (r.status().as_u16(), r.headers().clone());
    let body = r.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(body.to_vec()).unwrap(), headers)
}
fn head(html: &str) -> &str {
    &html[..html.find("</head>").unwrap()]
}
fn title(html: &str) -> &str {
    let s = html.find("<title>").unwrap() + 7;
    &html[s..s + html[s..].find("</title>").unwrap()]
}
fn description(html: &str) -> &str {
    let key = "<meta name=\"description\" content=\"";
    let s = html.find(key).unwrap() + key.len();
    &html[s..s + html[s..].find('"').unwrap()]
}
fn json_ld(html: &str) -> serde_json::Value {
    let start = html.find("<script type=\"application/ld+json\">").unwrap() + 35;
    let end = start + html[start..].find("</script>").unwrap();
    serde_json::from_str(&html[start..end]).unwrap()
}
fn canonical(html: &str) -> String {
    format!("<link rel=\"canonical\" href=\"{}\">", {
        let key = "<link rel=\"canonical\" href=\"";
        let s = html.find(key).unwrap() + key.len();
        &html[s..s + html[s..].find('"').unwrap()]
    })
}
fn cards(html: &str) -> usize {
    html.matches("<article class=\"project-card\"").count()
}

#[tokio::test]
async fn redirects_and_not_found() {
    let f = fixture(4).await;
    let app = router(f.state);
    for (uri, status, location) in [
        ("/categories", 301, Some("/categories/")),
        (
            "/categories/developer-tools",
            301,
            Some("/categories/developer-tools/"),
        ),
        (
            "/categories/developer-tools/page/2",
            301,
            Some("/categories/developer-tools/page/2/"),
        ),
        (
            "/categories/developer-tools/page/1/",
            301,
            Some("/categories/developer-tools/"),
        ),
        (
            "/categories/developer-tools/page/002/",
            301,
            Some("/categories/developer-tools/page/2/"),
        ),
        (
            "/categories/developer-tools/?utm_source=x",
            301,
            Some("/categories/developer-tools/"),
        ),
        (
            "/categories/developer-tools/?page=2",
            301,
            Some("/categories/developer-tools/page/2/"),
        ),
        (
            "/categories/developer-tools/?q=a&category=small",
            301,
            Some("/categories/developer-tools/"),
        ),
        (
            "/categories/developer-tools/page/9/",
            302,
            Some("/categories/developer-tools/page/2/"),
        ),
        ("/categories/unknown/", 404, None),
        ("/categories/health-fitness/", 404, None),
        ("/categories/developer-tools/extra/", 404, None),
        ("/categories/developer-tools/page/", 404, None),
        ("/categories/audiences/", 404, None),
    ] {
        let (code, _, headers) = get(&app, uri).await;
        assert_eq!(code, status, "{uri}");
        if let Some(location) = location {
            assert_eq!(headers[header::LOCATION], location, "{uri}");
        }
    }
}

#[tokio::test]
async fn landing_page_content_head_and_pagination() {
    let f = fixture(4).await;
    let app = router(f.state);
    let (code, html, headers) = get(&app, "/categories/developer-tools/").await;
    assert_eq!(code, 200);
    assert_eq!(title(&html), "Big Things projects · wrkgn");
    assert_eq!(description(&html), "Large projects of every kind.");
    assert_eq!(
        canonical(&html),
        "<link rel=\"canonical\" href=\"https://example.test/categories/developer-tools/\">"
    );
    assert!(!head(&html).contains("name=\"robots\""), "indexable");
    assert!(html.contains("<h1 id=\"directory-heading\">Big Things</h1>"));
    assert!(html.contains("<p class=\"landing-intro\">Large projects of every kind.</p>"));
    assert!(html.contains("<p data-total-count>35 projects</p>"));
    assert!(html.contains("data-list-url=\"/categories/developer-tools/\""));
    assert!(html.contains("<a href=\"/categories/developer-tools/page/2/\">Next →</a>"));
    assert!(head(&html).contains(
        "<meta property=\"og:url\" content=\"https://example.test/categories/developer-tools/\">"
    ));
    assert!(
        head(&html)
            .contains("<meta name=\"twitter:title\" content=\"Big Things projects · wrkgn\">")
    );
    let og = format!("https://example.test{}", wrkgn_web::render::OG_IMAGE_URL);
    for needle in [
        format!("<meta property=\"og:image\" content=\"{og}\">"),
        "<meta property=\"og:image:width\" content=\"1200\">".into(),
        "<meta property=\"og:image:height\" content=\"630\">".into(),
        "<meta property=\"og:image:alt\" content=\"wrkgn \u{2014} Projects shared on Hacker News\">".into(),
        "<meta name=\"twitter:card\" content=\"summary_large_image\">".into(),
    ] {
        assert!(head(&html).contains(&needle), "{needle}");
    }
    assert_eq!(cards(&html), 30);
    // Newest first.
    assert!(html.find("Project 35").unwrap() < html.find("Project 6<").unwrap());
    let ld = json_ld(&html);
    assert_eq!(ld["@type"], "CollectionPage");
    assert_eq!(ld["name"], "Big Things projects");
    assert_eq!(
        ld["url"],
        "https://example.test/categories/developer-tools/"
    );
    assert_eq!(ld["description"], "Large projects of every kind.");
    // Fixed label is checked in the sidebar so controls lead to the query result.
    assert!(html.contains("value=\"developer-tools\" data-filter-input=\"category\" checked"));
    assert!(!html.contains("value=\"games\" data-filter-input=\"category\" checked"));
    // Public cache class, not Filtered (which has no stale-while-revalidate).
    assert!(
        headers["cloudflare-cdn-cache-control"]
            .to_str()
            .unwrap()
            .contains("stale-while-revalidate")
    );

    let (code, html, _) = get(&app, "/categories/developer-tools/page/2/").await;
    assert_eq!(code, 200);
    assert_eq!(title(&html), "Big Things projects – Page 2 · wrkgn");
    assert_eq!(description(&html), "Large projects of every kind. Page 2.");
    assert_eq!(
        canonical(&html),
        "<link rel=\"canonical\" href=\"https://example.test/categories/developer-tools/page/2/\">"
    );
    assert!(
        !head(&html).contains("name=\"robots\""),
        "page 2 is index, follow"
    );
    assert!(html.contains("data-list-url=\"/categories/developer-tools/page/2/\""));
    assert!(html.contains("<a href=\"/categories/developer-tools/\">← Previous</a>"));
    assert_eq!(cards(&html), 5);
    assert_eq!(
        json_ld(&html)["url"],
        "https://example.test/categories/developer-tools/page/2/"
    );
}

#[tokio::test]
async fn hostile_taxonomy_text_is_escaped() {
    let f = fixture(4).await;
    let app = router(f.state);
    let (_, html, _) = get(&app, "/categories/games/").await;
    assert!(!html.contains("<script>alert(1)"));
    assert!(!html.contains("</script><script>"));
    assert!(description(&html).chars().count() <= 155);
    assert!(
        json_ld(&html)["description"]
            .as_str()
            .unwrap()
            .contains("alert(1)")
    );
    assert!(html.contains("<p data-total-count>3 projects</p>"));
}

#[tokio::test]
async fn index_page() {
    let f = fixture(4).await;
    let app = router(f.state);
    let (code, html, headers) = get(&app, "/categories/").await;
    assert_eq!(code, 200);
    assert_eq!(title(&html), "Categories · wrkgn");
    assert_eq!(
        canonical(&html),
        "<link rel=\"canonical\" href=\"https://example.test/categories/\">"
    );
    assert!(!head(&html).contains("name=\"robots\""));
    assert_eq!(json_ld(&html)["@type"], "CollectionPage");
    assert!(html.contains("<a href=\"/categories/developer-tools/\">Big Things</a>"));
    assert!(html.contains("<span class=\"landing-index__count\">35 projects</span>"));
    assert!(html.contains("Large projects of every kind."));
    assert!(
        headers["cloudflare-cdn-cache-control"]
            .to_str()
            .unwrap()
            .contains("stale-while-revalidate")
    );
    // A taxonomy label with no projects is not listed.
    assert!(!html.contains("/categories/health-fitness/"));
    assert!(html.contains("<a href=\"/categories/\">Categories</a>"));
}

#[tokio::test]
async fn query_results_stay_noindex_and_canonicalise_home() {
    let f = fixture(4).await;
    let app = router(f.state);
    let (code, html, headers) = get(&app, "/?category=developer-tools").await;
    assert_eq!(code, 200);
    assert!(head(&html).contains("<meta name=\"robots\" content=\"noindex, follow\">"));
    assert_eq!(
        canonical(&html),
        "<link rel=\"canonical\" href=\"https://example.test/\">"
    );
    assert!(
        !headers["cloudflare-cdn-cache-control"]
            .to_str()
            .unwrap()
            .contains("stale-while-revalidate")
    );
    let (_, html, _) = get(&app, "/").await;
    assert!(!head(&html).contains("noindex"));
    assert_eq!(cards(&html), 30);
}

#[tokio::test]
async fn tag_links_sit_beside_the_project_link() {
    let f = fixture(4).await;
    let app = router(f.state);
    let (_, html, _) = get(&app, "/categories/games/").await;
    let card = &html[html.find("<article class=\"project-card\"").unwrap()..];
    let card = &card[..card.find("</article>").unwrap()];
    let link = &card[card.find("<a data-project-link").unwrap()..];
    let link = &link[..link.find("</a>").unwrap()];
    assert!(!link.contains("tag"));
    assert!(card.contains("<a class=\"tag tag--link\" href=\"/categories/games/\">Small</a>"));
    assert!(!card.contains("<span class=\"tag\">"));
    let (_, html, _) = get(&app, "/projects/p1/").await;
    assert!(html.contains(
        "<a class=\"tag tag--link\" href=\"/categories/developer-tools/\">Big Things</a>"
    ));
    assert!(!html.contains("data-project-link"));
}

#[tokio::test]
async fn detail_and_list_routes_unaffected() {
    let f = fixture(4).await;
    let app = router(f.state);
    assert_eq!(get(&app, "/projects/p1/").await.0, 200);
    assert_eq!(get(&app, "/projects/").await.0, 301);
    assert_eq!(get(&app, "/projects/page/2/").await.0, 200);
    assert_eq!(get(&app, "/projects/absent/").await.0, 404);
    let (_, html, _) = get(&app, "/").await;
    assert!(html.contains("What are folks working on?"));
    assert!(html.contains("<p data-total-count>38 projects published</p>"));
}

#[tokio::test]
async fn saturated_landing_routes_return_503() {
    let f = fixture(1).await;
    let permit = f.state.rendering.acquire().await.unwrap();
    let app = router(f.state.clone());
    for uri in [
        "/categories/",
        "/categories/developer-tools/",
        "/categories/developer-tools/page/2/",
        "/audiences/",
        "/audiences/developers/",
        "/audiences/developers/page/2/",
    ] {
        let (code, _, headers) = get(&app, uri).await;
        assert_eq!(code, 503, "{uri}");
        assert_eq!(headers[header::RETRY_AFTER], "1");
        assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    }
    drop(permit);
    assert_eq!(get(&app, "/categories/developer-tools/").await.0, 200);
}

#[tokio::test]
async fn sitemap_order_and_membership() {
    let f = fixture(4).await;
    let app = router(f.state);
    let (_, xml, _) = get(&app, "/sitemap.xml").await;
    let entries: Vec<&str> = xml.split("<url>").skip(1).collect();
    // 38 projects: 2 list pages, category index + 2, audience index + 2, 38 projects.
    assert_eq!(entries.len(), 2 + 3 + 3 + 38);
    let locs: Vec<&str> = entries
        .iter()
        .map(|e| {
            e.split("</loc>")
                .next()
                .unwrap()
                .trim_start_matches("<loc>")
        })
        .collect();
    assert_eq!(
        locs[..8],
        [
            "https://example.test/",
            "https://example.test/projects/page/2/",
            "https://example.test/categories/",
            "https://example.test/categories/developer-tools/",
            "https://example.test/categories/games/",
            "https://example.test/audiences/",
            "https://example.test/audiences/developers/",
            "https://example.test/audiences/educators/",
        ]
    );
    assert!(locs[8].starts_with("https://example.test/projects/p"));
    assert!(entries[..8].iter().all(|e| !e.contains("<lastmod>")));
    assert!(entries[8..].iter().all(|e| e.contains("<lastmod>")));
    assert!(!xml.contains("/audiences/developers/page/2/"));
    assert!(!xml.contains("/audiences/gamers/"));
    assert!(!xml.contains("/categories/developer-tools/page/2/"));
    assert!(!xml.contains("/categories/health-fitness/"));
}

#[tokio::test]
async fn sitemap_split_boundary_keeps_fixed_order() {
    let f = fixture(4).await;
    let path = f.dir.path().join("public.sqlite");
    let conn = public::open_read_only(&path).unwrap();
    let landing: Vec<String> = [
        "/categories/",
        "/categories/developer-tools/",
        "/categories/games/",
        "/audiences/",
        "/audiences/developers/",
        "/audiences/educators/",
    ]
    .map(String::from)
    .into();
    let origin = "https://example.test";
    // 2 + 3 + 3 + 38 = 46 URLs, split at 4 per file: the landing entries straddle files.
    let limit = 4;
    let index = sitemap(&conn, origin, 38, &landing, None, limit)
        .unwrap()
        .unwrap();
    assert_eq!(index.matches("<sitemap>").count(), 12);
    let mut all = vec![];
    for part in 1..=12 {
        let xml = sitemap(&conn, origin, 38, &landing, Some(part), limit)
            .unwrap()
            .unwrap();
        let n = xml.matches("<url>").count();
        assert_eq!(n, if part < 12 { limit } else { 46 - 44 }, "part {part}");
        all.extend(
            xml.split("<loc>")
                .skip(1)
                .map(|e| e.split("</loc>").next().unwrap().to_owned()),
        );
    }
    assert_eq!(all.len(), 46);
    assert_eq!(all[2], format!("{origin}/categories/"));
    assert_eq!(all[4], format!("{origin}/categories/games/"));
    assert_eq!(all[5], format!("{origin}/audiences/"));
    assert_eq!(all[7], format!("{origin}/audiences/educators/"));
    assert!(all[8].contains("/projects/p"));
    let unique: std::collections::BTreeSet<_> = all.iter().collect();
    assert_eq!(unique.len(), 46);
    assert!(
        sitemap(&conn, origin, 38, &landing, Some(13), limit)
            .unwrap()
            .is_none()
    );
    // An exact multiple needs no empty trailing file.
    assert!(
        sitemap(&conn, origin, 38, &landing, Some(2), 46)
            .unwrap()
            .is_none()
    );
    assert!(
        sitemap(&conn, origin, 38, &landing, Some(1), 46)
            .unwrap()
            .is_none()
    );
    assert!(
        sitemap(&conn, origin, 38, &landing, None, 46)
            .unwrap()
            .unwrap()
            .contains("<urlset")
    );
}

#[tokio::test]
async fn audience_redirects_and_not_found() {
    let f = fixture(4).await;
    let app = router(f.state);
    for (uri, status, location) in [
        ("/audiences", 301, Some("/audiences/")),
        ("/audiences/developers", 301, Some("/audiences/developers/")),
        (
            "/audiences/developers/page/2",
            301,
            Some("/audiences/developers/page/2/"),
        ),
        (
            "/audiences/developers/page/1/",
            301,
            Some("/audiences/developers/"),
        ),
        (
            "/audiences/developers/page/002/",
            301,
            Some("/audiences/developers/page/2/"),
        ),
        (
            "/audiences/developers/?utm_source=x",
            301,
            Some("/audiences/developers/"),
        ),
        (
            "/audiences/developers/?page=2",
            301,
            Some("/audiences/developers/page/2/"),
        ),
        (
            "/audiences/developers/?q=a&audience=educators",
            301,
            Some("/audiences/developers/"),
        ),
        (
            "/audiences/developers/page/9/",
            302,
            Some("/audiences/developers/page/2/"),
        ),
        ("/audiences/unknown/", 404, None),
        ("/audiences/gamers/", 404, None),
        ("/audiences/developers/extra/", 404, None),
        ("/audiences/developers/page/", 404, None),
        ("/audiences/categories/", 404, None),
        ("/categories/developers/", 404, None),
        ("/audiences/developer-tools/", 404, None),
    ] {
        let (code, _, headers) = get(&app, uri).await;
        assert_eq!(code, status, "{uri}");
        assert_eq!(
            headers.get(header::LOCATION).map(|v| v.to_str().unwrap()),
            location,
            "{uri}"
        );
    }
}

#[tokio::test]
async fn audience_landing_page_one_and_two() {
    let f = fixture(4).await;
    let app = router(f.state);
    let (code, html, headers) = get(&app, "/audiences/developers/").await;
    assert_eq!(code, 200);
    assert_eq!(title(&html), "Projects for Coders · wrkgn");
    assert_eq!(
        canonical(&html),
        "<link rel=\"canonical\" href=\"https://example.test/audiences/developers/\">"
    );
    assert!(!head(&html).contains("name=\"robots\""));
    assert_eq!(description(&html), "People who write software.");
    assert_eq!(cards(&html), 30);
    assert!(html.contains("data-list-url=\"/audiences/developers/\""));
    assert!(html.contains("<a href=\"/audiences/developers/page/2/\">Next →</a>"));
    assert!(html.contains("<p data-total-count>35 projects</p>"));
    assert_eq!(json_ld(&html)["@type"], "CollectionPage");
    assert_eq!(json_ld(&html)["name"], "Projects for Coders");
    assert!(html.contains("value=\"developers\" data-filter-input=\"audience\" checked"));
    assert!(!html.contains("value=\"educators\" data-filter-input=\"audience\" checked"));
    assert!(
        headers["cloudflare-cdn-cache-control"]
            .to_str()
            .unwrap()
            .contains("stale-while-revalidate")
    );
    let (code, html, _) = get(&app, "/audiences/developers/page/2/").await;
    assert_eq!(code, 200);
    assert_eq!(title(&html), "Projects for Coders – Page 2 · wrkgn");
    assert_eq!(
        canonical(&html),
        "<link rel=\"canonical\" href=\"https://example.test/audiences/developers/page/2/\">"
    );
    assert!(!head(&html).contains("noindex"));
    assert_eq!(cards(&html), 5);
    assert!(html.contains("<a href=\"/audiences/developers/\">← Previous</a>"));
    // The smallest audience, with hostile description text, is escaped.
    let (code, html, _) = get(&app, "/audiences/educators/").await;
    assert_eq!(code, 200);
    assert!(!html.contains("<script>alert(1)"));
    assert!(html.contains("<p data-total-count>3 projects</p>"));
}

#[tokio::test]
async fn audience_index_page() {
    let f = fixture(4).await;
    let app = router(f.state);
    let (code, html, headers) = get(&app, "/audiences/").await;
    assert_eq!(code, 200);
    assert_eq!(title(&html), "Audiences · wrkgn");
    assert_eq!(
        canonical(&html),
        "<link rel=\"canonical\" href=\"https://example.test/audiences/\">"
    );
    assert!(!head(&html).contains("name=\"robots\""));
    assert!(html.contains("Browse projects shared on Hacker News by audience."));
    assert!(html.contains("<a href=\"/audiences/developers/\">Coders</a>"));
    assert!(html.contains("<a href=\"/audiences/educators/\">Teachers</a>"));
    assert!(html.contains("<span class=\"landing-index__count\">35 projects</span>"));
    assert!(!html.contains("/audiences/gamers/"));
    assert!(html.contains("<a href=\"/audiences/\">Audiences</a>"));
    assert!(
        headers["cloudflare-cdn-cache-control"]
            .to_str()
            .unwrap()
            .contains("stale-while-revalidate")
    );
}

#[tokio::test]
async fn audience_query_results_stay_noindex_and_canonicalise_home() {
    let f = fixture(4).await;
    let app = router(f.state);
    let (code, html, headers) = get(&app, "/?audience=developers").await;
    assert_eq!(code, 200);
    assert!(head(&html).contains("<meta name=\"robots\" content=\"noindex, follow\">"));
    assert_eq!(
        canonical(&html),
        "<link rel=\"canonical\" href=\"https://example.test/\">"
    );
    assert!(
        !headers["cloudflare-cdn-cache-control"]
            .to_str()
            .unwrap()
            .contains("stale-while-revalidate")
    );
}

#[tokio::test]
async fn audience_tag_links_are_on_project_pages_only() {
    let f = fixture(4).await;
    let app = router(f.state);
    let (_, html, _) = get(&app, "/projects/p1/").await;
    assert!(html.contains("<a class=\"tag tag--link\" href=\"/audiences/developers/\">Coders</a>"));
    assert!(!html.contains("<span class=\"tag\">"));
    assert!(!html.contains("data-project-link"));
    // Cards carry category tags only.
    let (_, html, _) = get(&app, "/audiences/educators/").await;
    let card = &html[html.find("<article class=\"project-card\"").unwrap()..];
    let card = &card[..card.find("</article>").unwrap()];
    assert!(!card.contains("/audiences/"));
    assert!(card.contains("href=\"/categories/games/\""));
}

#[tokio::test]
async fn navigation_links_audiences() {
    let f = fixture(4).await;
    let app = router(f.state);
    let (_, html, _) = get(&app, "/").await;
    assert_eq!(
        html.matches("<a href=\"/audiences/\">Audiences</a>")
            .count(),
        3
    );
}
