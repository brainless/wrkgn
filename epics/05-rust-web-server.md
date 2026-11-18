# Epic 05: Rust web server and server-side discovery

Status: Implementation delivered (the Astro site and JSON export it kept for transition were removed on 2026-11-18, so the Astro comparisons below are historical) through six sequential agent tasks. Asset builds and compilation checks passed. The maintainer manually tested the server and reported that it looks good, with the filtered project ordering difference below deferred. No agent review, test-suite execution, parity runs, or performance measurements were performed. Remaining validation is documented in the [implementation record](../docs/implementation/epic05.md).

## Known issue deferred

During manual testing, the maintainer observed projects appearing in different positions when filters are applied compared with the npm-based Astro app in `website/`. Investigate filtered ordering parity in a later follow-up; the cause has not been established. The maintainer explicitly deferred this issue rather than requiring a fix for this implementation. With Astro removed, no parity reference remains.

Depends on: [Epic 02](02-structured-extraction-and-public-export.md), [Epic 03](03-static-directory-and-discovery.md), and [Epic 04](04-show-hn-project-discovery.md). [Epic 06](06-vps-hosting-and-cloudflare-edge.md) deploys this server.

## Outcome

A Rust web server renders the project directory as HTML from a read-only public SQLite snapshot, with server-side search, filtering, sorting, and pagination. It replaces the Astro static build while preserving public URLs, page content, attribution rules, and discovery semantics. Responses carry explicit caching headers and canonical URLs, so Cloudflare can cache pages at the edge (Epic 06). TypeScript adds progressive enhancement; every page and discovery state still works without JavaScript.

The server is also the foundation for later community features (accounts, posting, project claims, moderation). Those features are out of scope here, but this epic's boundaries must not block them.

## Motivation

The approved snapshot holds 11,287 projects. The static build produces about 23,000 files, which exceeds Cloudflare's 20,000-file limit for free static hosting. Every visitor downloads an 11.8 MB `discovery.json` (2.3 MB gzipped) for browser-side search. Each refresh commits about 57 MB of snapshot data to git. All three grow with every month and every Show HN backfill. Server rendering from SQLite removes all three limits.

## Product decisions

- This epic replaces the PRD and AGENTS.md rule of "no hosted database or request-time API initially". Rewrite those sections as part of this epic; do not append contradictory alternatives. Until Epic 06's cutover, the documentation describes the Astro site as the production site and `wrkgn-web` as its locally verified replacement.
- **Data boundary:** the pipeline, working database, credentials, raw payloads, and review history stay on the maintainer's computer. The server receives only an allowlisted public snapshot.
- **Two databases:** the public snapshot is a separate, immutable SQLite file that is replaced as a whole on each publish. Later community data lives in a separate database owned by the server, and publishing never modifies it. Community records refer to projects by their stable IDs and slugs.
- **Search moves to the server.** `wrkgn-web` never serves the browser-wide discovery dataset or per-project detail JSON. The export keeps writing them only for the Astro site until Epic 06 retires it.
- **Discovery runs in memory.** The server loads the compact discovery fields from the snapshot into memory and ports the existing JavaScript matching and sorting to Rust. This keeps search semantics identical to the current site, including non-ASCII text and one- and two-character queries. The current snapshot averages about 330 characters of searchable text per project (about 4 MB in total), so a linear scan stays fast even at 100,000 projects. SQLite serves detail pages, related projects, and the sitemap. An FTS index is added only if measurement shows the scan misses the latency budget.
- **The Astro site stays deployable** until Epic 06 completes the cutover. This epic does not delete `website/`.

## Scope

### Crate layout

1. Turn the repository root into a Cargo workspace and keep the existing pipeline package at the root.
   - Add a small shared crate for the public snapshot: the schema, version constants, typed records, and validation. The exporter writes `public.sqlite` through it and the server reads through it, so the schema has one definition.
   - Add a `wrkgn-web` server crate that depends on the shared crate but not on Rig, model providers, or HN collection.
   - This keeps the server's dependency graph small for cross-compiling to Linux (Epic 06).
   - Keep the existing binaries, commands, and the pinned toolchain unchanged. Cranelift remains the dev/test backend through the existing `[profile.dev]` and `[profile.test]` settings; release builds use the default LLVM backend.

### Public SQLite snapshot

2. Advance the public schema to version 6 for the whole export. Schema 6 adds `public.sqlite` to each snapshot directory, next to the existing JSON files.
   - The manifest gains a `database` entry naming `public.sqlite`. `current.json` keeps its shape and selects the snapshot directory.
   - Change the version digest's schema prefix to `public-schema-6`. Otherwise an existing snapshot directory with an unchanged version would be skipped and would never receive `public.sqlite`.
   - Update `website/lib/public-snapshot.mjs` to accept schema 6 and the new manifest entry, so `npm run stage` and the Astro build keep working. Staging still re-serializes only the validated JSON and never copies `public.sqlite` into `website/`.
   - Document the schema in `docs/development.md`.
3. Build `public.sqlite` from the same consistent read transaction and the same allowlisted public model as the JSON.
   - Running export twice from unchanged data produces the same snapshot version. The version remains a digest of the public model, not of the database bytes.
   - Contents, limited to what the pages render plus stable identifiers:
     - A metadata table: public schema version, snapshot version, export time, and the category and audience taxonomies with labels and definitions.
     - Projects: ID, slug, name, description, problem, audience text, `latest_shared_at`, and comment count.
     - Mentions in date order, with name, description, problem, audience, source item ID and URL, anchor author, sharing date, and links.
     - Evidence records with source field, verbatim value, actual source author, date, and link.
     - Builder usernames, category labels, and audience labels per project, indexed for same-builder lookups.
     - The searchable text per project, built exactly as the JSON export builds it.
   - It never contains raw payloads, private review or processing state, failure diagnostics, mention keys, authorization fingerprints, or evidence support mappings.
4. Write `public.sqlite` to a temporary file inside the staging directory and finalize it (no WAL or journal files, `VACUUM`, integrity check) before the staging directory is renamed and `current.json` is updated. An interrupted export must never leave a partial database selected.
   - The shared validation rejects: a wrong schema version, a snapshot version that differs from the pointer, a missing metadata table, an integrity-check failure, missing indexes, unknown category or audience slugs, duplicate IDs or slugs, unsafe slugs, non-`http(s)` links, and any table or column outside the allowlist.
5. Keep the JSON snapshot until Epic 06 retires the Astro site. Then remove JSON generation for the discovery dataset and per-project details, unless another consumer has been added.

### Server

6. Build the server with `axum` on `tokio`.
   - Open the snapshot read-only with a small connection pool. SQLite calls must not block the async workers, so run them on blocking threads or a dedicated pool.
   - Listen on `127.0.0.1` by default. Configure the address, snapshot root, public origin, and cache lifetimes through command-line arguments or environment. No secrets are needed in this epic.
   - The public origin (for example `https://wrkgn.com`) builds absolute canonical links and sitemap URLs. Staging hostnames therefore point canonical links at production.
   - Compress HTML and text responses with gzip or Brotli. `Vary: Accept-Encoding` is the only `Vary` header.
   - Limit concurrent list and search rendering. When saturated, answer `503` with `Retry-After` and a `no-store` response, so unique search queries that miss the edge cache cannot exhaust the shared VPS.
7. The snapshot root contains `current.json` and `snapshots/<version>/`, as `wrkgn export` writes them. At startup, load and validate the database the pointer selects before serving.
   - On `SIGHUP`, open and validate the newly selected snapshot, build its in-memory discovery index, and switch atomically. In-flight requests finish on the old snapshot.
   - An invalid new snapshot is rejected, logged, and the previous one keeps serving.
   - `wrkgn-web check <path>` runs the same validation offline against a snapshot root or a single `public.sqlite`.
8. Routes, preserving existing public URLs and the `trailingSlash: 'always'` behavior:
   - `/` is the first list page. `/projects/` redirects permanently to `/`. Later pages are `/projects/page/N/`; `/projects/page/1/` redirects permanently to `/`.
   - `/projects/<slug>/` is a standalone project page. Unknown slugs return a real 404 page.
   - The list routes accept query parameters `q`, `category` (repeatable), `audience` (repeatable), `sort`, and `page`, with the current names and values from `website/src/lib/discovery-state.mjs`.
   - `/robots.txt`.
   - `/sitemap.xml`, split into a sitemap index when it exceeds 50,000 URLs. It lists unfiltered list pages and project pages, but not filtered URLs.
   - `/healthz` reports the build identifier and snapshot version and is never cached. The build identifier comes from a compile-time environment variable that Epic 06's deploy script sets; local builds report `dev`.
   - Static assets are served at content-hashed paths.
   - Legacy `/data/<version>/...` JSON URLs return 404.
   - Page paths without a trailing slash redirect permanently to the slashed form, keeping the query string. `/robots.txt`, `/sitemap.xml`, sitemap parts, `/healthz`, and asset paths have no trailing slash.
   - `GET` and `HEAD` are supported; other methods return `405`.
9. Render HTML with compile-time templates. `askama` is recommended because its syntax ports cleanly from the Astro components.
   - Port `SiteLayout`, `ProjectListing`, `ProjectCard`, `ProjectDetail`, and `DiscoveryControls`, plus `global.css`, keeping the current visual design: typography, fonts, card grid, sidebar, mobile filter overlay, and footer notice.
   - Detail pages keep Epic 03 and Epic 04 presentation rules:
     - Dated mentions, with the anchor author and sharing date kept distinct from evidence authors and dates.
     - Title evidence rendered as a title rather than a comment excerpt.
     - Builder attribution shown as a convention, not verified ownership.
     - Classifications kept distinct from source statements, and summaries from quotations.
     - Links to other projects by the same builder, in discovery order (newest first, then project ID).
   - Escape all source-derived text, and link only `http(s)` URLs.
   - Keep the independence notice. Do not present a placeholder as a working correction or removal contact.
   - Send `Content-Security-Policy` allowing scripts only from the site's own asset paths, styles and fonts from the site and Google Fonts, and no framing. Move the inline `<noscript>` style and all inline scripts into asset files so no inline code needs an exception. Also send `X-Content-Type-Options: nosniff` and `Referrer-Policy: strict-origin-when-cross-origin`.
10. Embed static assets (CSS, compiled TypeScript, icons) in the binary so a deploy is one file. Fonts continue to load from Google Fonts.

### Discovery semantics

11. Preserve the existing discovery semantics exactly, porting `readState`, `matchingProjects`, `optionsFrom`, and `stateUrl` from `website/src/lib/discovery-state.mjs`:
    - Search is a case-insensitive substring match over each project's searchable text (name, description, problem, audience text, and builder usernames). Lowercase with Unicode default case mapping (Rust `to_lowercase`), which matches JavaScript `toLowerCase` and the current `toLocaleLowerCase` outside locale-specific mappings.
    - Filter values are OR within Category or Audience, and AND across dimensions and with the query.
    - The four sort orders keep their current tie-breakers: for comment sorts, newest date, then ascending project ID; for date sorts, ascending project ID. Comment counts are always present in schema 6, so the legacy unknown-count ordering is dropped.
    - Results are sorted before pagination, 30 per page.
    - Filter options come from the published data and are ordered by slug.
    - Empty results show the current empty states.
    - Page state follows Epic 03, except that out-of-range pages redirect to the last available page instead of clamping silently.
12. Hold the discovery fields in memory per loaded snapshot: ID, slug, card fields, lowercased searchable text, category and audience sets, date, and comment count. Precompute the four sort orders once per snapshot, so a request filters a presorted list and stops after the requested page while still counting all matches.
    - Cap the query at 100 characters after trimming; longer queries canonicalize to the truncated form, cut on a character boundary.
    - Measure memory use and query latency against the full snapshot and a synthetic 100,000-project snapshot. Add an FTS5 trigram index only if the scan misses the budget, and then keep the substring scan for queries shorter than three characters.
13. Canonicalize list URLs on the server by porting the `readState`/`stateUrl` rules to Rust, so the edge cache and crawlers see one URL per distinct result. Canonicalization covers:
    - parameter order, sorted and de-duplicated filter values, and dropping unknown or invalid values;
    - omitting the default sort and `page=1`;
    - trimming whitespace in `q` (inner whitespace is significant to substring matching and is preserved), and the length cap;
    - moving `/?page=N` without criteria to `/projects/page/N/`.

    Rules that do not depend on the data (parameter order, duplicates, default sort, `page=1`, whitespace, the length cap, invalid sort or page syntax, and moving `/?page=N`) redirect with `301`. Rules that depend on the loaded snapshot (filter values absent from the published options, and out-of-range pages) redirect with `302`, because a later snapshot can make the same URL valid.

### Caching headers

14. Classify every response and set its caching headers explicitly. Values are configurable; the defaults below are a starting point.

    | Response | Browser (`Cache-Control`) | Edge (`Cloudflare-CDN-Cache-Control`) |
    | --- | --- | --- |
    | Project pages, unfiltered list pages, sitemap, robots | about 60 s | about 7 days, with `stale-while-revalidate` and `stale-if-error` |
    | Filtered or sorted list pages without `q` | about 60 s | about 1 day |
    | Search results with `q` | short | about 1 hour |
    | Hashed assets | 1 year, `immutable` | 1 year, `immutable` |
    | Permanent (`301`) redirects | about 1 day | about 7 days |
    | Data-dependent (`302`) redirects | short | short |
    | 404 responses | short | short |
    | `503` load shedding and `/healthz` | `no-store` | `no-store` |

15. Give cacheable pages an `ETag` derived from the snapshot version, build identifier, and canonical URL, and answer `If-None-Match` with `304`. Compare with the weak comparison that `If-None-Match` requires, because Cloudflare weakens strong ETags when it recompresses responses.
16. Edge-cacheable responses never set cookies and never vary on cookies. Pages are identical for every visitor. Later personalized features load through separate uncached routes (for example `/api/*` and account routes) after the page loads.
17. Keep crawlers away from discovery combinations.
    - Filtered and search pages carry `<meta name="robots" content="noindex, follow">` and a canonical link to the unfiltered list.
    - `robots.txt` disallows query-string list URLs and references the sitemap.

### TypeScript progressive enhancement

18. Add a small TypeScript and CSS source directory for `wrkgn-web` outside `website/` (for example `crates/wrkgn-web/assets/`), with its own `package.json`. `esbuild` compiles and minifies it into a gitignored build directory that Cargo embeds.
    - The Node toolchain is needed only on the maintainer's computer, never on the server.
    - The server crate's build script fails with a clear message if the built assets are missing. Document the order everywhere Rust checks are listed: build assets first, then run cargo. This includes the AGENTS.md validation commands.
19. Without JavaScript:
    - Search, filters, and sort are an ordinary GET form that submits to `/`; the server redirects to the canonical URL.
    - Pagination and cards are ordinary links.
    - The filter controls and sort selector render on the server on every list and detail page, so the Epic 03 rule that controls appear only after browser data loads no longer applies.
20. With JavaScript:
    - Debounced search and immediate filter or sort changes compute the canonical URL in the browser, fetch it, and replace the results region and count. The canonical URL rules in TypeScript and Rust are tested against the same shared cases. If the server still redirects (for example to the last page), use the final response URL.
    - The first change in a search session adds a history entry; further keystrokes in the same session replace it. Filter, sort, and page changes each add an entry. Back and forward restore the state.
    - Use the same URLs as ordinary navigation, never a separate fragment parameter or request header, so JavaScript navigation shares the edge cache entries.
    - Keep the mobile filter overlay behavior from Epic 03: full-screen opaque panel, pinned Clear all / Close, scroll locking, focus restoration, and reset on resize.
    - On a failed fetch, navigate to the canonical URL.
21. Keep the project modal:
    - Opening a card fetches the project page HTML, extracts its detail markup, and shows it in an accessible modal while the URL changes to the project URL. The detail markup carries the snapshot version; a mismatch with the list page falls back to standalone navigation.
    - Closing restores the list URL, scroll position, and focus.
    - Back and forward reopen or close the modal.
    - Modified clicks open the standalone page. A failed fetch navigates to the standalone page.

### Logging and development

22. Log requests to stdout (journald on the VPS) with method, path, status, and duration.
    - Do not log query strings, cookies, or client IP addresses in this epic.
    - Log snapshot loads and reload failures with versions.
23. Local development:
    - `cargo run -p wrkgn-web -- --snapshot local-data/public-export` serves the current export on localhost.
    - An asset watch command rebuilds TypeScript and CSS.
    - Update `docs/development.md`, `MANAGE.md`, AGENTS.md, and the PRD for the new commands, schema 6, data contract, and architecture. Document that `npm run stage` and the Astro workflow remain only until Epic 06's cutover.

## Maintainer workflow

```text
collect → process → review / approve → export (JSON + public.sqlite) → build assets → run wrkgn-web locally → inspect
```

The maintainer inspects the server locally against the current export. Deployment to the VPS is Epic 06. Until cutover, the existing Astro site remains the production site and is still staged and published from the JSON snapshot.

## Acceptance criteria

- `wrkgn export` produces schema 6 with a valid, deterministic `public.sqlite` from the same transaction as the JSON snapshot. It contains only allowlisted public fields, and an interrupted export never selects a partial database. `npm run stage` and the Astro build accept schema 6.
- `wrkgn-web` serves every existing public URL: `/`, `/projects/` (by redirect), `/projects/page/N/`, and `/projects/<slug>/` for every published project. The content matches the Astro output for the same snapshot: names, descriptions, dates, attribution, evidence, links, categories, audiences, and same-builder links.
- Search, filters, sort, and pagination return the same projects in the same order as the current browser implementation, on shared fixtures and on the full snapshot. The documented exceptions are redirects for out-of-range pages and the query length cap.
- Every list state has exactly one canonical URL. Non-canonical variants redirect to it with `301` for data-independent rules and `302` for data-dependent rules.
- Every response class carries the documented caching and security headers. No edge-cacheable response sets or varies on a cookie. `304` revalidation works with strong and weak `If-None-Match` values.
- With JavaScript disabled, all browsing, search, filtering, sorting, pagination, and project pages work. With JavaScript, the results update in place and the modal and mobile filter overlay behave as in Epic 03.
- A new snapshot is picked up on `SIGHUP` without dropping requests. An invalid snapshot is rejected and the previous one keeps serving.
- The server binary has no Rig, model-provider, or HN collection dependencies and needs no credentials.
- Server-side list and search requests on the full snapshot meet a documented latency budget measured locally (for example p99 under 50 ms), and the same holds for a synthetic 100,000-project snapshot. Resident memory with two snapshots loaded during a reload is documented for Epic 06's service limits.

## Validation

- **Rust tests:** build the assets, then run `cargo fmt --check`, `cargo clippy --all-targets`, and `cargo test` across the workspace. Use offline fixtures: port the representative and empty website fixtures to `public.sqlite` form, plus a generated large fixture.
- **Website tests:** run `npm test` and `npm run build` in `website/` to confirm the Astro site still accepts schema 6.
- **Route tests:**
  - status codes, redirects (`301` versus `302`), trailing slashes, `HEAD`, and `405`;
  - canonicalization cases: ordering, duplicates, invalid values, default sort, `page=1`, `/projects/page/1/`, out-of-range pages, whitespace, over-long queries;
  - caching and security headers per class, `ETag` and `304`, and the absence of `Set-Cookie`;
  - load shedding returns `503` with `no-store`;
  - escaping of hostile source text and rejection of non-`http(s)` links;
  - 404 pages, `robots.txt`, and the sitemap.
- **Semantic parity:** port the cases in `website/tests/discovery.test.mjs` to Rust, including non-ASCII and one- and two-character queries. A maintainer-run parity script runs the existing JavaScript implementation over the staged JSON and compares its results with the server on the full snapshot, covering every sort with filters, queries, and later pages. Ordinary tests use only fixtures.
- **Canonical URL parity:** the Rust and TypeScript canonicalization tests read the same JSON case file.
- **Content parity:** compare the rendered project pages and list pages against the Astro build of the same snapshot using normalized text, not byte-identical HTML.
- **Publication boundary:** inspect `public.sqlite`'s schema and contents against the allowlist.
- **Snapshot reload:** test valid, invalid, and concurrent-request cases.
- **Performance:** record latency and memory for the full and synthetic 100,000-project snapshots.
- **Manual browser testing by the maintainer**, with no browser automation. Use the manual checklist from `docs/development.md`, updated for server rendering:
  - wide and narrow layouts, and the mobile filter overlay;
  - JavaScript disabled;
  - search, filter, and sort in-place updates;
  - back and forward;
  - modal open, close, refresh, and modified click;
  - direct project URLs;
  - no Content Security Policy violations in the browser console.

Record which commands and checks were actually run.

## Boundaries and handoff

This epic delivers the server, the public SQLite snapshot, and local development. It does not include:

- VPS setup, Cloudflare Tunnel, cache rules, deployment or publish scripts, DNS cutover, or removal of the Astro site and tracked snapshot data. These are all in [Epic 06](06-vps-hosting-and-cloudflare-edge.md).
- Accounts, sessions, posting, claims, moderation, a community database, or Litestream backups.
- Changes to extraction, matching, or review.
