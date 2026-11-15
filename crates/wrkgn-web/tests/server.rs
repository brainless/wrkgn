//! Offline route/lifecycle fixtures; manual browser checks remain separate.
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use std::{path::Path, sync::Arc};
use tower::ServiceExt;
use wrkgn_public_snapshot::{
    self as public, Detail, Discovery, Evidence, Mention, Metadata, Taxonomy,
};
use wrkgn_web::{
    config::Config,
    server::{AppState, router},
};
fn fixture(path: &Path, version: &str) {
    let categories = [
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
    let audiences = [
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
    let taxonomy = |slugs: &[&str]| {
        slugs
            .iter()
            .map(|s| Taxonomy {
                slug: s.to_string(),
                label: s.to_string(),
                description: s.to_string(),
            })
            .collect()
    };
    let metadata = Metadata {
        schema_version: 6,
        version: version.into(),
        exported_at: 100,
        categories: taxonomy(&categories),
        audiences: taxonomy(&audiences),
    };
    let project = Discovery {
        id: 1,
        slug: "sample".into(),
        name: Some("<script>unsafe</script>".into()),
        description: "A sample project".into(),
        problem: None,
        audience: None,
        builder_usernames: vec!["builder".into()],
        categories: vec!["productivity".into()],
        audience_labels: vec![],
        latest_shared_at: 100,
        comment_count: 2,
        searchable_text: "sample café".into(),
    };
    let detail = Detail {
        id: 1,
        slug: project.slug.clone(),
        name: project.name.clone(),
        mentions: vec![Mention {
            name: project.name.clone(),
            description: project.description.clone(),
            problem: None,
            audience: None,
            source_item_id: 1,
            source_url: "https://news.ycombinator.com/item?id=1".into(),
            author: Some("builder".into()),
            shared_at: 100,
            source_excerpt: "sample".into(),
            builder_usernames: vec!["builder".into()],
            links: vec!["https://example.com/".into()],
            evidence: vec![Evidence {
                source_item_id: 1,
                source_field: "body".into(),
                value: "sample".into(),
                author: Some("builder".into()),
                shared_at: 100,
                source_url: "https://news.ycombinator.com/item?id=1".into(),
            }],
        }],
    };
    public::write_database(path, &metadata, &[project], &[detail]).unwrap();
}
async fn state() -> (tempfile::TempDir, AppState) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("public.sqlite");
    fixture(&path, "0123456789abcdef");
    let config = Config {
        snapshot: path,
        render_limit: 1,
        ..Config::default()
    };
    (dir, AppState::load(config).await.unwrap())
}
#[tokio::test]
async fn routes_headers_methods_and_weak_revalidation() {
    let (_dir, state) = state().await;
    let app = router(state);
    for (uri, status, location) in [
        ("/projects/", 301, Some("/")),
        ("/projects/sample", 301, Some("/projects/sample/")),
        ("/?sort=date-desc", 301, Some("/")),
        ("/?category=unknown", 302, Some("/")),
        ("/projects/page/9/", 302, Some("/")),
        ("/projects/absent/", 404, None),
        ("/data/old/discovery.json", 404, None),
    ] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), status, "{uri}");
        if let Some(location) = location {
            assert_eq!(response.headers()[header::LOCATION], location);
        }
        assert_eq!(
            response.headers()[header::X_CONTENT_TYPE_OPTIONS],
            "nosniff"
        );
        assert_eq!(response.headers()[header::VARY], "Accept-Encoding");
        assert!(!response.headers().contains_key(header::SET_COOKIE));
        assert!(
            response
                .headers()
                .contains_key("cloudflare-cdn-cache-control")
        );
    }
    let response = app
        .clone()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let etag = response.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_owned();
    for candidate in [
        etag.clone(),
        format!("W/{etag}"),
        format!("\"other\", W/{etag}"),
        "*".into(),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::IF_NONE_MATCH, candidate)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert!(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .is_empty()
        );
    }
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/")
                .method("HEAD")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .is_empty()
    );
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/")
                .method("POST")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(response.headers()[header::ALLOW], "GET, HEAD");
    for uri in ["/robots.txt", "/sitemap.xml", "/healthz"] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        if uri == "/healthz" {
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        }
    }
}
#[tokio::test]
async fn saturation_and_snapshot_lifetime() {
    let (dir, state) = state().await;
    let app = router(state.clone());
    let permit = state.rendering.acquire().await.unwrap();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/?q=sample")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()[header::RETRY_AFTER], "1");
    drop(permit);
    let old = state.snapshot.load_full();
    let next = dir.path().join("next.sqlite");
    fixture(&next, "fedcba9876543210");
    // Atomic publish of a new file leaves old pooled connections usable.
    std::fs::rename(next, &state.config.snapshot).unwrap();
    state.reload().await.unwrap();
    assert_eq!(state.snapshot.load().metadata.version, "fedcba9876543210");
    assert_eq!(old.metadata.version, "0123456789abcdef");
    assert_eq!(
        old.read(public::project_slugs).await.unwrap(),
        vec!["sample"]
    );
    let held = Arc::clone(&old);
    let concurrent = tokio::spawn(async move { held.read(public::project_slugs).await.unwrap() });
    let invalid = dir.path().join("invalid.sqlite");
    std::fs::write(&invalid, "invalid").unwrap();
    std::fs::rename(invalid, &state.config.snapshot).unwrap();
    assert!(state.reload().await.is_err());
    assert_eq!(state.snapshot.load().metadata.version, "fedcba9876543210");
    assert_eq!(concurrent.await.unwrap(), vec!["sample"]);
}
