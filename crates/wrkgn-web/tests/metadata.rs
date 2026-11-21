//! Head metadata, JSON-LD and sitemap checks over an offline fixture.
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
    server::{AppState, router},
};

const LONG: &str = "MIB: a self-hosted tool that watches your build pipelines and explains failures in plain language, so small teams spend less time reading logs and more time shipping fixes to production.";
const HOSTILE: &str = "x </script><script>alert(1)</script> \"quoted\" 'single' <b>&amp; café</b>";

fn taxonomy(slugs: &[&str]) -> Vec<Taxonomy> {
    slugs
        .iter()
        .map(|s| Taxonomy {
            slug: s.to_string(),
            label: s.to_string(),
            description: s.to_string(),
        })
        .collect()
}
fn project(id: i64, slug: &str, name: Option<&str>, description: &str, at: i64) -> Discovery {
    Discovery {
        id,
        slug: slug.into(),
        name: name.map(Into::into),
        description: description.into(),
        problem: None,
        audience: None,
        builder_usernames: vec![],
        categories: vec![],
        audience_labels: vec![],
        latest_shared_at: at,
        comment_count: 0,
        searchable_text: format!("{slug} {description}"),
    }
}
fn mention(p: &Discovery, shared_at: i64) -> Mention {
    Mention {
        name: p.name.clone(),
        description: p.description.clone(),
        problem: None,
        audience: None,
        source_item_id: p.id,
        source_url: format!("https://news.ycombinator.com/item?id={}", p.id),
        author: Some("alice".into()),
        shared_at,
        source_excerpt: "excerpt".into(),
        builder_usernames: p.builder_usernames.clone(),
        links: vec![],
        evidence: vec![Evidence {
            source_item_id: p.id,
            source_field: "body".into(),
            value: "excerpt".into(),
            author: Some("alice".into()),
            shared_at,
            source_url: format!("https://news.ycombinator.com/item?id={}", p.id),
        }],
    }
}
async fn app() -> (tempfile::TempDir, axum::Router) {
    let mut projects = vec![
        project(1, "mib", Some("mib"), LONG, 1_700_000_000),
        project(2, "unnamed", None, "Unnamed project tracker", 1_690_000_000),
        project(
            3,
            "hostile",
            Some("Evil \"n\" </script>"),
            HOSTILE,
            1_680_000_000,
        ),
        project(
            4,
            "cafe",
            Some("Café"),
            "CAFÉ - ünïcödé ".repeat(10).trim(),
            1_670_000_000,
        ),
    ];
    projects[0].builder_usernames = vec!["alice".into(), "a b&c".into()];
    for i in 5..40 {
        projects.push(project(
            i,
            &format!("p{i}"),
            Some(&format!("P{i}")),
            "filler",
            1_600_000_000 + i,
        ));
    }
    let details: Vec<Detail> = projects
        .iter()
        .map(|p| Detail {
            id: p.id,
            slug: p.slug.clone(),
            name: p.name.clone(),
            // Newest first, so the earliest date is last (and not first).
            mentions: vec![
                mention(p, p.latest_shared_at),
                mention(p, p.latest_shared_at - 86_400 * 10),
                mention(p, p.latest_shared_at - 86_400 * 400),
            ],
        })
        .collect();
    let metadata = Metadata {
        schema_version: 6,
        version: "0123456789abcdef".into(),
        exported_at: 100,
        categories: taxonomy(&[
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
        ]),
        audiences: taxonomy(&[
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
        ]),
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("public.sqlite");
    public::write_database(&path, &metadata, &projects, &details).unwrap();
    let config = Config {
        snapshot: path,
        public_origin: "https://example.test".into(),
        ..Config::default()
    };
    (dir, router(AppState::load(config).await.unwrap()))
}
async fn get(app: &axum::Router, uri: &str) -> (u16, String, axum::http::HeaderMap) {
    let r = app
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = r.status().as_u16();
    let headers = r.headers().clone();
    let body = r.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(body.to_vec()).unwrap(), headers)
}
fn assert_og_image(h: &str) {
    let url = format!("https://example.test{}", wrkgn_web::render::OG_IMAGE_URL);
    for needle in [
        format!("<meta property=\"og:image\" content=\"{url}\">"),
        format!("<meta name=\"twitter:image\" content=\"{url}\">"),
        "<meta property=\"og:image:width\" content=\"1200\">".into(),
        "<meta property=\"og:image:height\" content=\"630\">".into(),
        "<meta property=\"og:image:alt\" content=\"wrkgn \u{2014} Projects shared on Hacker News\">".into(),
        "<meta name=\"twitter:card\" content=\"summary_large_image\">".into(),
    ] {
        assert!(h.contains(&needle), "{needle}");
    }
}
fn head(html: &str) -> &str {
    &html[..html.find("</head>").unwrap()]
}
fn json_ld(html: &str) -> serde_json::Value {
    let start = html.find("<script type=\"application/ld+json\">").unwrap() + 35;
    let end = start + html[start..].find("</script>").unwrap();
    serde_json::from_str(&html[start..end]).unwrap()
}
fn title(html: &str) -> String {
    let s = html.find("<title>").unwrap() + 7;
    html[s..s + html[s..].find("</title>").unwrap()].to_owned()
}

#[tokio::test]
async fn list_pages_have_social_tags_and_distinct_titles() {
    let (_d, app) = app().await;
    let (_, home, _) = get(&app, "/").await;
    let h = head(&home);
    assert_eq!(title(&home), "Projects shared on Hacker News · wrkgn");
    for needle in [
        "<meta property=\"og:site_name\" content=\"wrkgn\">",
        "<meta property=\"og:type\" content=\"website\">",
        "<meta property=\"og:url\" content=\"https://example.test/\">",
        "<link rel=\"canonical\" href=\"https://example.test/\">",
        "<meta name=\"twitter:card\" content=\"summary_large_image\">",
        "<meta property=\"og:title\" content=\"Projects shared on Hacker News · wrkgn\">",
    ] {
        assert!(h.contains(needle), "{needle}");
    }
    assert!(!h.contains("noindex"));
    assert_og_image(h);
    let ld = json_ld(&home);
    assert_eq!(ld["@type"], "WebSite");
    assert_eq!(ld["url"], "https://example.test/");
    assert!(ld.get("potentialAction").is_none());

    let (status, page, _) = get(&app, "/projects/page/2/").await;
    assert_eq!(status, 200);
    let h = head(&page);
    assert_eq!(
        title(&page),
        "Projects shared on Hacker News – Page 2 · wrkgn"
    );
    assert!(
        h.contains("<meta property=\"og:url\" content=\"https://example.test/projects/page/2/\">")
    );
    assert!(h.contains("<link rel=\"canonical\" href=\"https://example.test/projects/page/2/\">"));
    assert!(h.contains("twitter:title") && !h.contains("ld+json"));
    assert_og_image(h);
    assert!(h.contains("Page 2."));

    for uri in ["/?q=mib", "/?category=productivity"] {
        let (_, page, _) = get(&app, uri).await;
        // Unknown-option redirects are covered elsewhere; q result must be noindex.
        if uri.starts_with("/?q") {
            assert!(head(&page).contains("<meta name=\"robots\" content=\"noindex, follow\">"));
            assert_eq!(title(&page), "Search results · wrkgn");
            assert!(!head(&page).contains("ld+json"));
        }
    }
}

#[tokio::test]
async fn project_page_title_description_and_json_ld() {
    let (_d, app) = app().await;
    let (status, html, _) = get(&app, "/projects/mib/").await;
    assert_eq!(status, 200);
    let t = title(&html);
    assert!(t.starts_with("mib – a self-hosted tool"), "{t}");
    assert!(t.ends_with("… · wrkgn") && t.chars().count() <= 60, "{t}");
    let h = head(&html);
    assert!(h.contains("<meta property=\"og:type\" content=\"article\">"));
    assert_og_image(h);
    assert!(
        h.contains("<meta property=\"og:url\" content=\"https://example.test/projects/mib/\">")
    );
    // Meta description truncated; visible lead paragraph is full.
    let start = h.find("<meta name=\"description\" content=\"").unwrap() + 34;
    let desc = &h[start..start + h[start..].find('"').unwrap()];
    assert!(desc.chars().count() <= 155 && desc.ends_with('…'), "{desc}");
    for tag in ["og:description", "twitter:description"] {
        assert!(h.contains(&format!("content=\"{desc}\"")) && h.contains(tag));
    }
    assert!(html.contains(&format!("<p class=\"detail-lead\">{LONG}</p>")));
    assert!(html.contains("<h1>mib</h1>"));

    let ld = json_ld(&html);
    assert_eq!(ld["@type"], "CreativeWork");
    assert_eq!(ld["name"], "mib");
    assert_eq!(ld["description"], LONG);
    assert_eq!(ld["url"], "https://example.test/projects/mib/");
    // Earliest of the three mentions: latest - 400 days.
    let earliest = 1_700_000_000 - 86_400 * 400;
    assert_eq!(
        ld["datePublished"],
        wrkgn_web::metadata::utc(earliest).unwrap()
    );
    assert!(ld.get("dateModified").is_none() && !html.contains("dateModified"));
    let authors = ld["author"].as_array().unwrap();
    assert_eq!(authors.len(), 2);
    let url_of = |name: &str| {
        let a = authors.iter().find(|a| a["name"] == name).unwrap();
        assert_eq!(a["@type"], "Person");
        a["url"].as_str().unwrap().to_owned()
    };
    assert_eq!(
        url_of("alice"),
        "https://news.ycombinator.com/user?id=alice"
    );
    assert_eq!(
        url_of("a b&c"),
        "https://news.ycombinator.com/user?id=a+b%26c"
    );
}

#[tokio::test]
async fn unnamed_multibyte_and_name_prefixed_titles() {
    let (_d, app) = app().await;
    let (_, html, _) = get(&app, "/projects/unnamed/").await;
    assert_eq!(
        title(&html),
        "Unnamed project – Unnamed project tracker · wrkgn"
    );
    assert!(html.contains("<h1>Unnamed project</h1>"));
    assert_eq!(json_ld(&html)["name"], "Unnamed project");
    assert!(json_ld(&html).get("author").is_none());
    let (_, html, _) = get(&app, "/projects/cafe/").await;
    let t = title(&html);
    assert!(t.starts_with("Café – ünïcödé CAFÉ"), "{t}"); // only the leading name is stripped
    assert!(t.chars().count() <= 60, "{t}");
    let (_, html, _) = get(&app, "/projects/p7/").await;
    assert_eq!(title(&html), "P7 – filler · wrkgn");
}

#[tokio::test]
async fn hostile_source_text_cannot_escape_head_or_script() {
    let (_d, app) = app().await;
    let (_, html, _) = get(&app, "/projects/hostile/").await;
    let h = head(&html);
    assert_eq!(
        h.matches("</script>").count(),
        2,
        "module script and ld+json only"
    );
    assert!(!h.contains("<script>alert") && !h.contains("<b>"));
    assert!(!h.contains("content=\"x </script>"));
    assert!(
        h.contains("&#60;/script&#62;")
            && h.contains("&#34;quoted&#34;")
            && h.contains("&#38;amp;")
    );
    let ld = json_ld(&html);
    assert_eq!(ld["description"], HOSTILE);
    assert_eq!(ld["name"], "Evil \"n\" </script>");
    // The JSON-LD block itself contains no raw `<`.
    let start = h.find("application/ld+json\">").unwrap();
    let block = &h[start..h[start..].find("</script>").map(|i| start + i).unwrap()];
    assert!(!block.contains('<'));
}

#[tokio::test]
async fn errors_have_no_social_tags() {
    let (_d, app) = app().await;
    let (status, html, _) = get(&app, "/projects/absent/").await;
    assert_eq!(status, 404);
    assert!(!html.contains("og:") && !html.contains("twitter:") && !html.contains("ld+json"));
}

#[tokio::test]
async fn sitemap_lastmod_only_on_projects_and_url_set_unchanged() {
    let (_d, app) = app().await;
    let (_, xml, _) = get(&app, "/sitemap.xml").await;
    let entries: Vec<&str> = xml.split("<url>").skip(1).collect();
    // 39 projects -> 2 list pages, the category and audience indexes (no
    // project has either here) and 39 project URLs.
    assert_eq!(entries.len(), 43);
    for e in &entries {
        let project = e.contains("/projects/") && !e.contains("/projects/page/");
        assert_eq!(e.contains("<lastmod>"), project, "{e}");
        assert!(!e.contains("changefreq") && !e.contains("priority"));
    }
    assert!(entries[0].starts_with("<loc>https://example.test/</loc></url>"));
    assert!(entries[1].starts_with("<loc>https://example.test/projects/page/2/</loc></url>"));
    assert_eq!(
        entries[2],
        "<loc>https://example.test/categories/</loc></url>"
    );
    assert_eq!(
        entries[3],
        "<loc>https://example.test/audiences/</loc></url>"
    );
    assert_eq!(
        entries[4],
        "<loc>https://example.test/projects/mib/</loc><lastmod>2023-11-14T22:13:20Z</lastmod></url>"
    );
}

#[tokio::test]
async fn tracking_parameters_follow_documented_behavior() {
    let (_d, app) = app().await;
    let (status, _, headers) = get(&app, "/?utm_source=x").await;
    assert_eq!(status, 301);
    assert_eq!(headers[header::LOCATION], "/");
    let (status, html, _) = get(&app, "/projects/mib/?utm_source=x").await;
    assert_eq!(status, 200);
    assert!(
        head(&html)
            .contains("<link rel=\"canonical\" href=\"https://example.test/projects/mib/\">")
    );
}

#[tokio::test]
async fn fonts_are_self_hosted_and_preloaded() {
    use wrkgn_web::render::{
        FONT_DM_SANS_LATIN_EXT_URL, FONT_DM_SANS_URL, FONT_SOURCE_SERIF_4_LATIN_EXT_URL,
        FONT_SOURCE_SERIF_4_URL,
    };
    let (_d, app) = app().await;
    for uri in [
        "/",
        "/projects/page/2/",
        "/projects/mib/",
        "/projects/absent/",
    ] {
        let (_, html, headers) = get(&app, uri).await;
        assert!(
            !html.contains("fonts.googleapis.com") && !html.contains("fonts.gstatic.com"),
            "{uri}"
        );
        assert!(!html.contains("preconnect"), "{uri}");
        let csp = headers["content-security-policy"].to_str().unwrap();
        assert!(
            !csp.contains("googleapis") && !csp.contains("gstatic"),
            "{csp}"
        );
        assert!(
            csp.contains("style-src 'self';") && csp.contains("font-src 'self';"),
            "{csp}"
        );
        if uri != "/projects/absent/" {
            for font in [FONT_DM_SANS_URL, FONT_SOURCE_SERIF_4_URL] {
                let link = format!(
                    "<link rel=\"preload\" as=\"font\" type=\"font/woff2\" crossorigin href=\"{font}\">"
                );
                assert!(html.contains(&link), "{uri} {font}");
            }
            for ext in [
                FONT_DM_SANS_LATIN_EXT_URL,
                FONT_SOURCE_SERIF_4_LATIN_EXT_URL,
            ] {
                assert!(
                    !html.contains(ext),
                    "latin-ext must not be preloaded: {uri}"
                );
            }
        }
    }
}
