use crate::{
    config::{CacheConfig, Config},
    discovery::PAGE_SIZE,
    render,
    snapshot::Snapshot,
};
use arc_swap::ArcSwap;
use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::Response,
};
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Instant};
use tokio::sync::Semaphore;
use tower_http::compression::CompressionLayer;
use wrkgn_public_snapshot as public;

pub const BUILD_ID: &str = match option_env!("WRKGN_BUILD_ID") {
    Some(id) => id,
    None => "dev",
};
const SITEMAP_LIMIT: usize = 50_000;
const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self' https://fonts.googleapis.com; font-src 'self' https://fonts.gstatic.com; img-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'; object-src 'none'";
#[derive(Clone)]
pub struct AppState {
    pub snapshot: Arc<ArcSwap<Snapshot>>,
    pub config: Arc<Config>,
    pub rendering: Arc<Semaphore>,
}
impl AppState {
    pub async fn load(config: Config) -> Result<Self, String> {
        let path = config.snapshot.clone();
        let count = config.connections;
        let snapshot = tokio::task::spawn_blocking(move || Snapshot::load(&path, count))
            .await
            .map_err(|_| "snapshot loader failed")??;
        println!("snapshot loaded version={}", snapshot.metadata.version);
        Ok(Self {
            snapshot: Arc::new(ArcSwap::from_pointee(snapshot)),
            rendering: Arc::new(Semaphore::new(config.render_limit)),
            config: Arc::new(config),
        })
    }
    pub async fn reload(&self) -> Result<(), String> {
        let path = self.config.snapshot.clone();
        let count = self.config.connections;
        let next = tokio::task::spawn_blocking(move || Snapshot::load(&path, count))
            .await
            .map_err(|_| "snapshot loader failed")??;
        let version = next.metadata.version.clone();
        self.snapshot.store(Arc::new(next));
        println!("snapshot reloaded version={version}");
        Ok(())
    }
}
pub fn router(state: AppState) -> Router {
    Router::new()
        .fallback(handle)
        .with_state(state)
        .layer(CompressionLayer::new().gzip(true))
        .layer(middleware::from_fn(finish_response))
}
async fn finish_response(req: Request, next: Next) -> Response {
    let start = Instant::now();
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let head = method == Method::HEAD;
    let mut response = next.run(req).await;
    response
        .headers_mut()
        .insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    response.headers_mut().remove(header::SET_COOKIE);
    if head {
        *response.body_mut() = Body::empty();
    }
    // Debug formatting escapes control characters, avoiding forged log lines.
    println!(
        "method={} path={:?} status={} duration_ms={:.3}",
        method,
        path,
        response.status().as_u16(),
        start.elapsed().as_secs_f64() * 1000.0
    );
    response
}
#[derive(Clone, Copy)]
enum CacheClass {
    Public,
    Filtered,
    Search,
    Asset,
    Permanent,
    Temporary,
    Missing,
    NoStore,
}
fn response(
    status: StatusCode,
    content_type: &str,
    body: impl Into<Body>,
    class: CacheClass,
    config: &CacheConfig,
    version: &str,
    url: &str,
    request_headers: &HeaderMap,
) -> Response {
    let mut response = Response::new(body.into());
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(content_type).unwrap(),
    );
    let (browser, edge) = match class {
        CacheClass::Public => (
            format!("public, max-age={}", config.browser),
            format!(
                "public, max-age={}, stale-while-revalidate={}, stale-if-error={}",
                config.edge_public, config.stale_revalidate, config.stale_error
            ),
        ),
        CacheClass::Filtered => (
            format!("public, max-age={}", config.browser),
            format!("public, max-age={}", config.edge_filtered),
        ),
        CacheClass::Search => (
            format!("public, max-age={}", config.browser_search),
            format!("public, max-age={}", config.edge_search),
        ),
        CacheClass::Asset => (
            format!("public, max-age={}, immutable", config.asset),
            format!("public, max-age={}, immutable", config.asset),
        ),
        CacheClass::Permanent => (
            format!("public, max-age={}", config.browser_redirect),
            format!("public, max-age={}", config.edge_redirect),
        ),
        CacheClass::Temporary | CacheClass::Missing => (
            format!("public, max-age={}", config.short),
            format!("public, max-age={}", config.short),
        ),
        CacheClass::NoStore => ("no-store".into(), "no-store".into()),
    };
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_str(&browser).unwrap(),
    );
    response.headers_mut().insert(
        "cloudflare-cdn-cache-control",
        HeaderValue::from_str(&edge).unwrap(),
    );
    if status == StatusCode::OK && !matches!(class, CacheClass::NoStore) {
        let tag = format!(
            "\"{:x}\"",
            Sha256::digest(format!("{version}\0{BUILD_ID}\0{url}"))
        );
        response
            .headers_mut()
            .insert(header::ETAG, HeaderValue::from_str(&tag).unwrap());
        if if_none_match(request_headers, &tag) {
            *response.status_mut() = StatusCode::NOT_MODIFIED;
            *response.body_mut() = Body::empty();
            response.headers_mut().remove(header::CONTENT_TYPE);
        }
    }
    response
}
/// If-None-Match uses weak comparison, including Cloudflare's recompressed W/ tags.
pub fn if_none_match(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|value| {
            value.split(',').any(|tag| {
                let tag = tag.trim();
                tag == "*"
                    || tag.strip_prefix("W/").unwrap_or(tag)
                        == etag.strip_prefix("W/").unwrap_or(etag)
            })
        })
}
fn redirect(
    status: StatusCode,
    location: &str,
    state: &AppState,
    snapshot: &Snapshot,
    headers: &HeaderMap,
    url: &str,
) -> Response {
    let class = if status == StatusCode::MOVED_PERMANENTLY {
        CacheClass::Permanent
    } else {
        CacheClass::Temporary
    };
    let mut response = response(
        status,
        "text/plain; charset=utf-8",
        "Redirecting",
        class,
        &state.config.cache,
        &snapshot.metadata.version,
        url,
        headers,
    );
    match HeaderValue::from_str(location) {
        Ok(value) => {
            response.headers_mut().insert(header::LOCATION, value);
            response
        }
        Err(_) => error(StatusCode::BAD_REQUEST, state, snapshot, headers, url),
    }
}
fn error(
    status: StatusCode,
    state: &AppState,
    snapshot: &Snapshot,
    headers: &HeaderMap,
    url: &str,
) -> Response {
    response(
        status,
        "text/html; charset=utf-8",
        render::error(status.as_u16()),
        if status == StatusCode::NOT_FOUND {
            CacheClass::Missing
        } else {
            CacheClass::NoStore
        },
        &state.config.cache,
        &snapshot.metadata.version,
        url,
        headers,
    )
}
fn list_path(path: &str) -> bool {
    path == "/"
        || path == "/projects/"
        || path
            .strip_prefix("/projects/page/")
            .and_then(|s| s.strip_suffix('/'))
            .is_some_and(|s| !s.is_empty() && !s.contains('/'))
}
async fn handle(State(state): State<AppState>, req: Request) -> Response {
    let snapshot = state.snapshot.load_full();
    let path = req.uri().path();
    let url = req
        .uri()
        .path_and_query()
        .map(|s| s.as_str())
        .unwrap_or(path);
    let headers = req.headers();
    if req.method() != Method::GET && req.method() != Method::HEAD {
        let mut response = error(
            StatusCode::METHOD_NOT_ALLOWED,
            &state,
            &snapshot,
            headers,
            url,
        );
        response
            .headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
        return response;
    }
    if path == "/healthz" {
        return response(
            StatusCode::OK,
            "application/json; charset=utf-8",
            serde_json::json!({"build":BUILD_ID,"snapshot":snapshot.metadata.version}).to_string(),
            CacheClass::NoStore,
            &state.config.cache,
            &snapshot.metadata.version,
            url,
            headers,
        );
    }
    if path.starts_with("/data/") {
        return error(StatusCode::NOT_FOUND, &state, &snapshot, headers, url);
    }
    if path.starts_with("/assets/") {
        return match render::asset(path) {
            Some((kind, bytes)) => response(
                StatusCode::OK,
                kind,
                bytes,
                CacheClass::Asset,
                &state.config.cache,
                &snapshot.metadata.version,
                path,
                headers,
            ),
            None => error(StatusCode::NOT_FOUND, &state, &snapshot, headers, url),
        };
    }
    if path == "/robots.txt" {
        let body = format!(
            "User-agent: *\nDisallow: /?\nDisallow: /projects/?\nDisallow: /projects/page/*?\nSitemap: {}/sitemap.xml\n",
            state.config.public_origin
        );
        return response(
            StatusCode::OK,
            "text/plain; charset=utf-8",
            body,
            CacheClass::Public,
            &state.config.cache,
            &snapshot.metadata.version,
            path,
            headers,
        );
    }
    if path == "/sitemap.xml" || path.starts_with("/sitemap-") {
        let part = if path == "/sitemap.xml" {
            None
        } else {
            match path
                .strip_prefix("/sitemap-")
                .and_then(|s| s.strip_suffix(".xml"))
                .and_then(|s| s.parse::<usize>().ok())
            {
                Some(n) if n > 0 && path == format!("/sitemap-{n}.xml") => Some(n),
                _ => return error(StatusCode::NOT_FOUND, &state, &snapshot, headers, url),
            }
        };
        let origin = state.config.public_origin.clone();
        let count = snapshot.discovery.projects.len();
        let result = snapshot
            .read(move |conn| sitemap(conn, &origin, count, part))
            .await;
        return match result {
            Ok(Some(body)) => response(
                StatusCode::OK,
                "application/xml; charset=utf-8",
                body,
                CacheClass::Public,
                &state.config.cache,
                &snapshot.metadata.version,
                path,
                headers,
            ),
            Ok(None) => error(StatusCode::NOT_FOUND, &state, &snapshot, headers, url),
            Err(_) => error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &state,
                &snapshot,
                headers,
                url,
            ),
        };
    }
    if !path.ends_with('/') && (path == "/projects" || path.starts_with("/projects/")) {
        let target = match req.uri().query() {
            Some(q) => format!("{path}/?{q}"),
            None => format!("{path}/"),
        };
        return redirect(
            StatusCode::MOVED_PERMANENTLY,
            &target,
            &state,
            &snapshot,
            headers,
            url,
        );
    }
    if list_path(path) {
        let Ok(permit) = Arc::clone(&state.rendering).try_acquire_owned() else {
            let mut response = error(
                StatusCode::SERVICE_UNAVAILABLE,
                &state,
                &snapshot,
                headers,
                url,
            );
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
            return response;
        };
        let worker_snapshot = Arc::clone(&snapshot);
        let path = path.to_owned();
        let query = req.uri().query().map(str::to_owned);
        let origin = state.config.public_origin.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let resolution = worker_snapshot.discovery.resolve(&path, query.as_deref());
            if let Some(redirect) = resolution.redirect {
                return Ok::<_, String>(Err(redirect));
            }
            let class = if !resolution.state.q.is_empty() {
                CacheClass::Search
            } else if resolution.state.has_criteria() {
                CacheClass::Filtered
            } else {
                CacheClass::Public
            };
            let body = render::listing(&worker_snapshot, &resolution, &origin)?;
            Ok(Ok((body, class, resolution.state.url())))
        })
        .await;
        return match result {
            Ok(Ok(Err(r))) => redirect(
                StatusCode::from_u16(r.status).unwrap(),
                &r.location,
                &state,
                &snapshot,
                headers,
                url,
            ),
            Ok(Ok(Ok((body, class, canonical)))) => response(
                StatusCode::OK,
                "text/html; charset=utf-8",
                body,
                class,
                &state.config.cache,
                &snapshot.metadata.version,
                &canonical,
                headers,
            ),
            _ => error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &state,
                &snapshot,
                headers,
                url,
            ),
        };
    }
    if let Some(slug) = path
        .strip_prefix("/projects/")
        .and_then(|s| s.strip_suffix('/'))
        .filter(|s| public::valid_slug(s))
    {
        let slug = slug.to_owned();
        let result = snapshot
            .read(move |conn| {
                let Some(detail) = public::detail(conn, &slug)? else {
                    return Ok(None);
                };
                let related = public::same_builder_projects(conn, detail.id)?;
                Ok(Some((detail, related)))
            })
            .await;
        return match result {
            Ok(Some((detail, related))) => {
                match render::project(&snapshot, &detail, &related, &state.config.public_origin) {
                    Ok(body) => response(
                        StatusCode::OK,
                        "text/html; charset=utf-8",
                        body,
                        CacheClass::Public,
                        &state.config.cache,
                        &snapshot.metadata.version,
                        path,
                        headers,
                    ),
                    Err(_) => error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        &state,
                        &snapshot,
                        headers,
                        url,
                    ),
                }
            }
            Ok(None) => error(StatusCode::NOT_FOUND, &state, &snapshot, headers, url),
            Err(_) => error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &state,
                &snapshot,
                headers,
                url,
            ),
        };
    }
    error(StatusCode::NOT_FOUND, &state, &snapshot, headers, url)
}
fn sitemap(
    conn: &rusqlite::Connection,
    origin: &str,
    count: usize,
    part: Option<usize>,
) -> Result<Option<String>, String> {
    let list_pages = count.div_ceil(PAGE_SIZE).max(1);
    let total = list_pages + count;
    let parts = total.div_ceil(SITEMAP_LIMIT);
    let origin = render::escape(origin);
    if part.is_none() && parts > 1 {
        let mut xml="<?xml version=\"1.0\" encoding=\"UTF-8\"?><sitemapindex xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">".to_owned();
        for n in 1..=parts {
            xml.push_str(&format!(
                "<sitemap><loc>{origin}/sitemap-{n}.xml</loc></sitemap>"
            ));
        }
        xml.push_str("</sitemapindex>");
        return Ok(Some(xml));
    }
    if part.is_some_and(|n| parts <= 1 || n > parts) {
        return Ok(None);
    }
    let start = (part.unwrap_or(1) - 1) * SITEMAP_LIMIT;
    let end = (start + SITEMAP_LIMIT).min(total);
    let mut xml="<?xml version=\"1.0\" encoding=\"UTF-8\"?><urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">".to_owned();
    for i in start..end.min(list_pages) {
        let path = if i == 0 {
            "/".into()
        } else {
            format!("/projects/page/{}/", i + 1)
        };
        xml.push_str(&format!("<url><loc>{origin}{path}</loc></url>"));
    }
    let offset = start.saturating_sub(list_pages);
    let limit = end.saturating_sub(start.max(list_pages));
    if limit > 0 {
        let mut stmt = conn
            .prepare(
                "SELECT slug FROM projects ORDER BY latest_shared_at DESC,id LIMIT ?1 OFFSET ?2",
            )
            .map_err(|e| e.to_string())?;
        let slugs = stmt
            .query_map([limit as i64, offset as i64], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        for slug in slugs {
            let slug = slug.map_err(|e| e.to_string())?;
            xml.push_str(&format!("<url><loc>{origin}/projects/{slug}/</loc></url>"));
        }
    }
    xml.push_str("</urlset>");
    Ok(Some(xml))
}
pub async fn serve(config: Config) -> Result<(), String> {
    let state = AppState::load(config).await?;
    #[cfg(unix)]
    {
        let reload_state = state.clone();
        let mut signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
            .map_err(|e| e.to_string())?;
        tokio::spawn(async move {
            while signal.recv().await.is_some() {
                if let Err(error) = reload_state.reload().await {
                    println!(
                        "snapshot reload rejected serving_version={} error={:?}",
                        reload_state.snapshot.load().metadata.version,
                        error
                    );
                }
            }
        });
    }
    let listener = tokio::net::TcpListener::bind(state.config.bind)
        .await
        .map_err(|e| e.to_string())?;
    println!(
        "listening address={} build={BUILD_ID}",
        listener.local_addr().map_err(|e| e.to_string())?
    );
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown())
        .await
        .map_err(|e| e.to_string())
}
async fn shutdown() {
    #[cfg(unix)]
    {
        if let Ok(mut term) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! {_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{}};
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
