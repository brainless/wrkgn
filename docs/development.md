# Local development

## Rust toolchain

This repository pins `nightly-2026-10-25` with `rustc-codegen-cranelift`, `rustfmt`, and `clippy`, matching the local `daftprompt` toolchain reference. Install it with:

```sh
rustup toolchain install nightly-2026-10-25 \
  --profile minimal \
  --component rustc-codegen-cranelift \
  --component rustfmt \
  --component clippy
```

Cargo's [`.cargo/config.toml`](../.cargo/config.toml) selects Cranelift for development and test profiles. The component alone does not select the backend. Check the installed components and selected backend from the repository root:

```sh
rustup show active-toolchain
rustup component list --installed
npm ci --prefix crates/wrkgn-web/assets
npm run build --prefix crates/wrkgn-web/assets
cargo build --workspace -v
```

The verbose build command should include `-Z codegen-backend=cranelift` in the `rustc` invocation. If Cargo reports an unsupported `codegen-backend` profile key, check that the pinned nightly is active and the component is installed.

Run local checks with:

```sh
npm run build --prefix crates/wrkgn-web/assets
cargo fmt --all --check
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

If a locally configured `sccache` Rust wrapper fails with a permission error, prefix these commands with `RUSTC_WRAPPER=` to run `rustc` directly.

## Local database

The storage library opens `local-data/wrkgn.sqlite` by default. Set `WRKGN_DB_PATH` to a different SQLite file when needed. Its parent directory is created automatically. The working database belongs outside any public website directory; the default `local-data/` path and `.env` files are ignored by Git.

SQL migrations in `migrations/` are applied transactionally using SQLite's `user_version`. The source tables retain raw HN payloads, item IDs, authors, original timestamps, parent IDs, sibling order, deleted/dead flags, and first/last fetch times. Each changed source snapshot gets a numbered revision; a repeat fetch with identical source data only advances its last fetch time. Content fingerprints cover the body, title, URL, author, original timestamp, and deleted/dead state, while child-list and score changes are excluded. The revision history still records raw payload changes.

Collection runs record their start, finish, fetched item count, and failure count. Failed and unavailable fetches are separate records and cannot mark an existing item deleted. A run with recorded failures finishes as `incomplete`. If a prior run was interrupted, the next run marks it incomplete with an interruption flag and starts a new full-tree refresh. This replay preserves previously fetched items and creates revisions only for changed source data.

Only a complete refresh can mark a previously stored item as absent from the current HN reply tree. This `present_in_tree` state is separate from the item's explicit `deleted` and `dead` flags and gets its own revision. An incomplete run leaves prior tree presence unchanged. When an item reappears, its presence and revision are restored. `Store::conversation_groups` returns date-preserving source records as ordered top-level comment subtrees; downstream processing must check the latest run is complete before using them.

## HN collection library

Schema version 4 repairs early version-three databases that lack `processing_failures`. Opening the database applies the upgrade automatically and preserves existing records. After a `no such table: processing_failures` error, rerun the original command with the updated CLI; no database reset is needed. That error can mask an extraction failure, which a subsequent run can now record and report.

`Collector` accepts a supplied thread item ID and an `HnApi` transport. The production transport reads the public HN item endpoint with a 15-second request timeout; tests inject offline fixture responses. The collector fetches the root and each listed descendant, preserving the `kids` order as sibling positions and validating child parent IDs. At most eight item fetches run concurrently by default. Transient network errors, HTTP 429, and HTTP 5xx responses receive up to three attempts with bounded delay.

A JSON `null`, malformed item, failed request, duplicate child reference, or parent mismatch makes the run incomplete and identifies the affected item in `fetch_failures`. An explicit `deleted` or `dead` field is stored as source state. A failed or null fetch never changes that state. No live collection runs during builds or tests; a deliberate live smoke test awaits a supplied monthly discussion ID.

## Processing and review

The CLI loads `.env` from its current working directory before choosing database and export paths. Copy [`.env.example`](../.env.example) to `.env` and fill in credentials, or set shell variables; shell values take precedence. A missing `.env` is fine, while malformed content stops the command. Keep credentials out of Git and logs.

`WRKGN_PROVIDER` defaults to `openai`. For OpenAI, set `OPENAI_API_KEY` and `WRKGN_MODEL` to an OpenAI completion model name. For Neuralwatt, set `WRKGN_PROVIDER=neuralwatt` and `NEURALWATT_API_KEY`; `WRKGN_MODEL` defaults to `glm-5.3-flash`, and `NEURALWATT_BASE_URL` defaults to `https://api.neuralwatt.com/v1`. Neuralwatt uses Rig's OpenAI-compatible chat route. Provider, model, and endpoint identify processed groups, so switching any of them triggers extraction again. Rig is pinned to upstream commit `7dfd8a4229a9d89af159489687cb14b404b2871d`. Model calls occur during an explicit `process` command or the separate `wrkgn-monthly` workflow. Tests use a deterministic `Model` fixture and never need credentials.

```sh
cargo run -- collect --thread <hn-item-id>   # only command that calls HN
cargo run -- process --thread <hn-item-id>
cargo run -- process --thread <hn-item-id> --reprocess
cargo run -- review pending
cargo run -- review approve 12-30 34
cargo run -- review correct <mention-key> description "Reviewed summary"
cargo run -- review suppress-source <hn-item-id> "removal request"
cargo run -- review suppress-project <project-id>
cargo run -- review match <from-project-id> <to-project-id>
cargo run -- export
```

`--db <path>` selects the local SQLite database. `--export <directory>` selects the public export directory; `WRKGN_EXPORT_PATH` overrides the default `local-data/public-export`. `process` never fetches from HN: it reads stored source and requires the latest collection run for the thread to be `complete`, otherwise it fails and asks you to rerun `collect`. Run `collect` first, then `process`. `--concurrency <n>` (default 10) sets how many conversation groups are extracted in parallel; lower it if the provider rate-limits. Rate-limit (429), provider 5xx, and connection errors are retried up to six attempts with jittered exponential backoff from 1s to 30s. After three groups fail with such errors (cumulative across the run), no new groups are started; already claimed groups finish, with further retries disabled after the stop signal; the rest are reported as not attempted, and rerunning without `--reprocess` resumes, skipping groups completed with the current prompt, schema, and model. Progress (`Extracting n/total groups (pct%), k failed`) goes to stderr, in place on a terminal and at 10% steps otherwise. On failure the CLI prints counts by error kind with one example group each; full errors are in the `processing_failures` table for the reported run. Results are written to SQLite on one thread in group order, so a group's result commits as soon as it and every earlier group are done. Unchanged, successful groups are skipped unless `--reprocess` is supplied. Prompt, model, schema, and source fingerprints determine whether a group is unchanged. A reply edit invalidates its top-level group. If one group fails, successful groups still commit, the failed group's previous result remains in SQLite, the processing run is marked incomplete, and the standalone `process` command exits nonzero. Oversized items or ancestor chains produce a recorded failure rather than truncated input.

Rig extraction uses a process-wide Tokio runtime that remains driven for the lifetime of its shared HTTP connection pool. A processing run coordinates retry cooldowns across its workers so that new attempts wait for the shared backoff deadline after transient failures; already running requests may finish. Rig internal retries are disabled so provider attempts pass through the shared cooldown. Transport diagnostics retain bounded error causes with the configured credential and endpoint redacted.

HN may reorder top-level comments between refreshes. A group's rank among other top-level comments does not change its extraction input and does not trigger a model call. Compatibility checks compare existing cache hashes against that root's previously collected positions, requiring the entire current subtree to otherwise match, then advance its fingerprint for publication without changing approvals. Existing cache hashes, prompt/schema versions, and model identities remain supported. Edits, additions, removals, or ordering changes within the reply subtree still invalidate the group. The collection report's changed-item count includes sibling-position and raw-payload changes; it is not a count of edited comments or required model calls.

Each model invocation receives one top-level comment subtree, or bounded chunks with root and ancestor context. All source IDs must be accounted for. HN HTML is converted to plain text with full link destinations before extraction and validation. Structured JSON output contains project mentions with a verbatim `source_excerpt`, a project-centric description, optional problem/audience text, category and audience slugs, and additional explicitly named `builders`. The source author is assumed to be a builder; `builders` excludes that author. Missing names may become short descriptive names. Replies provide context or introduce their own projects with their actual source IDs; requests and separate supporting contributions are no longer extracted.

Structured-output deserialization and local validation failures share up to two correction attempts per batch (`Options::max_validation_corrections`, default 2, capped at 2). Correction prompts retain the original source records and carry validation feedback and, when available, a bounded rejected response as untrusted JSON data. Every corrected response undergoes the same schema and provenance validation before any group results are saved. Exhausted validation errors record the source ID and bounded rejected field when available in the private `processing_failures.error` column; URL diagnostics omit credentials, queries, and fragments. Validation failures remain separate from provider failures even when rejected text contains transport-error wording. Retry-policy changes do not invalidate successful groups or require renewed approval by themselves.

Categories and audiences are proposed classifications. Audience slugs come from [audiences.md](audiences.md), with zero to five distinct values only when the source mentions the audience. The prompt and schema versions change so existing groups are processed again on the next explicit process run. Existing approved records are not silently reclassified or approved by a database migration. New extraction through the manual CLI still requires review; `wrkgn-monthly` automatically approves after each successful thread. URLs and excerpts are validated against the supplied source, including URLs attached to labels, footnotes, and punctuation wrappers; safe project-origin normalization is permitted for multiple same-origin demo URLs. Shared-hosting project paths must be preserved, and URLs embedded inside another URL do not count as separate source links.

New mentions are pending review. `review pending` prints one readable card per mention: extracted fields with corrections applied, labels, builders, links, and source evidence. Show HN cards display every stored evidence quotation with its source field, actual author, original date, HN source link, and supported fields. Authorized-builder evidence links to the submitter authorization source; its identifying quotation is also displayed among the evidence. Monthly cards retain the single-comment excerpt and context without invented field-level support. It ends with the pending IDs as compact ranges. Set `COLUMNS` to change the card width; `--json` retains the existing one-object-per-line fields, applies corrections, and adds an `evidence` array (empty for legacy monthly mentions) with source metadata and optional authorization source IDs. Review output does not include private fingerprints, raw payloads, or processing diagnostics. `approve` takes IDs and inclusive ranges (`12-30 34`) and approves them in one transaction. Non-pending IDs inside a range are skipped. A range with no pending mentions fails without approving anything. Review the cards before approving a range; corrections persist by mention key across reruns. Successful reprocessing returns changed material to pending review. Source and project suppression survive reruns. There is no command to undo suppression yet; inspect and edit local SQLite deliberately if a suppression was erroneous.

## Automated monthly binary

`cargo run --bin wrkgn-monthly --` discovers and handles monthly discussions one thread at a time, reusing `Collector`, `process_thread_with`, thread-scoped approval, and the standard public exporter. It defaults to the current UTC month through 23 months earlier. Use `--from YYYY-MM --through YYYY-MM` for an inclusive range, plus the existing `--db`, `--export`, and `--concurrency` settings. `default-run = "wrkgn"` keeps existing `cargo run -- ...` commands unchanged.

Discovery queries the HN Algolia story-title index separately for each full and abbreviated month name plus year, pages through all hits, filters whole titles locally, and deduplicates IDs. It accepts three-letter abbreviations (also `Sept`), optional `Ask HN:`, and question marks. Query word removal and prefix matching are disabled. An estimated hit count (`exhaustiveNbHits=false`) is accepted; counts exceeding the available pagination window or unexpectedly empty pages fail explicitly instead of silently dropping results. Network errors and 429/5xx responses receive up to three attempts. Indexing delays can hide new discussions until a later run. Empty months are skipped; all matching threads are processed before advancing to the previous month.

The collector refreshes the full tree so new replies, edits, and deletions retain existing semantics. The processor skips unchanged successful groups using its existing source/prompt/schema/model checks. After a complete successful thread pass, its pending mentions are automatically approved, including earlier pending results; other threads are unaffected. Corrections, matching, and suppression persist. Export remains cumulative across all approved threads. Thread failures are logged immediately with their month and thread ID, and traversal continues with the remaining threads and months. A month discovery failure is logged and skips that month’s unknown threads while older months are still attempted. After traversal, an aggregate summary identifies every failed month or thread and the command exits nonzero if any failed. Setup/configuration, database-open, and output-write failures remain fatal. The provider-failure cutoff still stops new groups within the affected thread, without stopping monthly traversal; reruns resume saved successful groups. A partial processing pass attempts a freshness-filtered export but does not auto-approve the thread. Approval and export commit separately, so rerun after an export failure. The workflow never stages website assets. See [the maintainer runbook](../MANAGE.md#automated-monthly-workflow).

Offline tests inject paginated discovery responses, HN fixtures, and a deterministic model; ordinary checks never call live HN or LLM services.

## Public snapshot contract

Schema 6 adds `public.sqlite` next to the JSON files in each completed snapshot,
with `database: "public.sqlite"` in its manifest. `current.json` retains its three
fields and uses schema version 6. The version digest begins with
`public-schema-6` and hashes the public records and taxonomy metadata, never the
SQLite bytes or export time. JSON remains available for Astro until cutover and the stable-period cleanup.
Staging accepts schemas 2–6 and copies only validated, re-serialized JSON; it
never copies `public.sqlite` into the website.

The `wrkgn-public-snapshot` workspace crate owns public record types, schema and
validation. SQLite has `metadata`, `taxonomy`, `projects`, `project_builders`,
`project_labels`, `mentions`, `mention_builders`, `mention_links`, and `evidence`
tables, with indexes for discovery order, builder lookups, labels and mentions.
Project IDs/slugs and all displayed mention/evidence attribution are retained.
Evidence support mappings, authorization dependencies, private mention keys,
review/processing history and raw payloads are excluded. Metadata holds schema
version, snapshot version and export time; taxonomy rows hold slugs, labels and
definitions. The database is built from the same captured public model as JSON,
written to an unpublished temporary file, vacuumed and validated before the
snapshot directory and pointer are renamed. Validation checks the exact object
and column allowlists, required indexes, integrity and foreign keys, metadata,
fixed taxonomy slugs, unique identities, safe slugs, HTTP(S) links, HN source
identities and required dated evidence. Read-only consumers reject journal/WAL
sidecars and validate the selected version before use.

`export` reads all approved projects from one SQLite transaction, applies corrections and suppression, and creates `snapshots/<version>/manifest.json`, `public.sqlite`, `discovery.json`, and `projects/<slug>.json` under the export directory. `current.json` is replaced atomically and points to the completed snapshot. The version hashes both discovery and detail data. Astro should read `current.json` once per build, then read all JSON through its selected manifest. It must never copy the working database or raw payloads.

Discovery records have stable `id` and `slug`, card fields (`name`, `description`, `problem`, `audience`), `builder_usernames`, `categories`, `audience_labels`, `latest_shared_at`, `comment_count`, and `searchable_text`. They are ordered by latest original mention date, then project ID. Public schema 6 retains `comment_count` from version 4: distinct replies beneath a project’s published source comments, including nested replies, excluding deleted, dead, absent, and source-suppressed replies. Overlapping subtrees are deduplicated. Counts use stored source at export time; no HN or model request is needed. Comment vote counts are unavailable from HN. Detail records have `id`, `slug`, `name`, and dated `mentions`; each mention includes its source ID and URL, author, links, verbatim source excerpt, and builder usernames. Dates are Unix seconds from HN source records. A current source edit, deletion, missing item, or suppression hides affected content; a failed rerun does not republish stale group results. Rust HTML, modal HTML, and the transitional Astro HTML/JSON use this snapshot.

The maintainer has inspected many detail pages and the project list and considers extraction quality sufficient to offer the explicit automated monthly workflow. Schema validity alone does not establish extraction accuracy; manually inspect descriptions, labels, invitations, provenance, and grouping before publication.

## Rust web server (Epic 05)

`crates/wrkgn-web/` depends on `crates/public-snapshot/`, with no pipeline/Rig/HN dependency. It renders Askama templates from a read-only public SQLite pool on blocking threads. Discovery/search fields live in memory with four presorted ID orders. It serves HTML rather than discovery/detail JSON. Epic 06 provides separate VPS deployment scripts; these local commands never deploy. Live cutover remains pending.

From the repository root:

```sh
npm ci --prefix crates/wrkgn-web/assets
npm run build --prefix crates/wrkgn-web/assets
cargo run -p wrkgn-web -- check local-data/public-export
cargo run -p wrkgn-web -- --snapshot local-data/public-export
```

The server defaults to http://127.0.0.1:3000. `check` accepts a snapshot root or a single `public.sqlite` and performs startup validation offline. Existing schema-5 exports need a deliberate `cargo run --bin wrkgn -- export`; a model rerun is unnecessary solely to generate schema 6. The working database is never opened by the server.

Assets live in `crates/wrkgn-web/assets/`, compile with esbuild to ignored `dist/`, and embed into the binary at content-hashed paths. `npm run watch --prefix crates/wrkgn-web/assets` rebuilds after source edits; restart Cargo after a rebuild to embed changed assets. Build assets before every Rust build/check/test sequence. Missing assets produce an explicit Cargo error. Node is needed only during maintainer builds. Fonts remain on Google Fonts.

The snapshot root uses `current.json` and `snapshots/<version>/`. Startup rejects invalid snapshots. After a complete new export, `kill -HUP <wrkgn-web-pid>` validates/loads the pointer and atomically swaps snapshots; old requests retain old connections/data. A failed replacement logs an error and retains the previous version. `/healthz` reports compile-time `WRKGN_BUILD_ID` (default `dev`) and snapshot version, with `no-store`. Request logs on stdout contain method, path, status and duration, excluding queries, IPs and cookies.

All options accept CLI flags or `WRKGN_WEB_<UPPERCASE_NAME>` environment variables, replacing hyphens with underscores; flags win. The server does not load `.env` or require provider credentials.

| Flag | Default | Purpose |
| --- | --- | --- |
| `--bind` | `127.0.0.1:3000` | Listen address |
| `--snapshot` | `local-data/public-export` | Public snapshot root or SQLite file |
| `--public-origin` | `https://wrkgn.com` | Absolute canonical and sitemap origin |
| `--connections` | `4` | Read-only connections, 1–32 |
| `--render-limit` | `8` | Concurrent list/search renders, 1–1024 |
| `--browser-ttl` | `60` | Public/filtered HTML browser seconds |
| `--edge-public-ttl` | `604800` | Public HTML, robots and sitemap edge seconds |
| `--edge-filtered-ttl` | `86400` | Filtered/sorted, no-query edge seconds |
| `--browser-search-ttl` | `15` | Search browser seconds |
| `--edge-search-ttl` | `3600` | Search edge seconds |
| `--short-ttl` | `30` | Data-dependent redirects and 404 seconds |
| `--browser-redirect-ttl` | `86400` | Permanent redirect browser seconds |
| `--edge-redirect-ttl` | `604800` | Permanent redirect edge seconds |
| `--asset-ttl` | `31536000` | Immutable hashed asset seconds |
| `--stale-revalidate` | `86400` | Edge stale-while-revalidate seconds |
| `--stale-error` | `604800` | Edge stale-if-error seconds |

The public origin must be an HTTP(S) origin without credentials, path, query or fragment. Google Fonts is the only external styling/font dependency. Responses send CSP, nosniff and strict-origin-when-cross-origin; scripts/styles need no inline exceptions. Compression is gzip; `Vary` contains only `Accept-Encoding`, and responses never set cookies. Cacheable representations have snapshot/build/canonical-URL ETags and support weak `If-None-Match`. Saturated list rendering returns 503 with Retry-After and no-store.

Routes preserve slashed page URLs. `/projects/` and `/projects/page/1/` redirect to `/`; unslashed pages redirect with query preserved. GET/HEAD are supported; other methods return 405. Legacy `/data/...` is 404. `/robots.txt` disallows query-string discovery URLs. `/sitemap.xml` lists unfiltered pages and details, splitting into `/sitemap-N.xml` parts at 50,000 URLs. Criteria pages are noindex and canonicalize to the unfiltered list.

Canonicalization runs syntax-only 301 rules first, then snapshot-dependent 302 rules. Search uses Unicode lowercase substring matching and a trimmed 100-character cap; inner whitespace remains significant. OR within filters, AND across dimensions/query, 30 results per page, with the existing four sort tie-breakers. Unknown published options and out-of-range pages use temporary redirects. Rust and TypeScript consume `crates/wrkgn-web/tests/fixtures/canonical-urls.json` in their respective test definitions.

### Maintainer parity and performance commands

These tools are explicit local checks, not automatic startup/build behavior. They use only public JSON/SQLite and local HTTP. They have been added but were not run during implementation. Start the server against the same schema-6 snapshot as the staged JSON, then run from the repository root:

```sh
npm test --prefix crates/wrkgn-web/assets
node crates/wrkgn-web/tools/discovery-parity.mjs website/data/public http://127.0.0.1:3000
python3 crates/wrkgn-web/tools/content-parity.py --snapshot website/data/public --astro website/dist --origin http://127.0.0.1:3000
```

Build Astro from that same staged snapshot before content parity (`npm run build --prefix website`). Semantic parity invokes the existing JavaScript implementation over all sorts, every individual published filter, combined filters, Unicode/short queries and first/second/last pages. Content parity compares normalized card/detail text and URLs, excluding the added builder-convention sentence and typography differences. Default coverage is every unfiltered list and project page; `--limit 100` limits detail coverage during an initial check. Differences fail with affected URLs for maintainer inspection.

For performance, build optimized binaries and create a new public-only fixture directory; generation refuses to overwrite existing paths and copies taxonomy from a public JSON manifest. No working SQLite database is read:

```sh
npm run build --prefix crates/wrkgn-web/assets
cargo build --release -p wrkgn-web --examples --bin wrkgn-web
cargo run --release -p wrkgn-web --example synthetic_snapshot -- website/data/public /tmp/wrkgn-synthetic-100k 100000
node crates/wrkgn-web/tools/performance.mjs target/release/wrkgn-web local-data/public-export /tmp/wrkgn-synthetic-100k 1000
cargo run --release -p wrkgn-web --example memory_probe -- local-data/public-export
cargo run --release -p wrkgn-web --example memory_probe -- /tmp/wrkgn-synthetic-100k
```

The benchmark binds localhost port 3099, runs full and synthetic snapshots sequentially, warms requests, and reports loopback end-to-end p50/p95/p99 plus RSS and sampled reload peak in JSON. The acceptance target is p99 under 50 ms for each dataset. Requests are sequential to measure ordinary service latency; separately exercise overload/load shedding. RSS uses `ps` in KiB and polling can miss peaks. `memory_probe` deliberately retains two complete snapshots and reports resident memory after one and two loads, providing a conservative starting point for Epic 06 service limits alongside observed reload peaks. Validation/loading temporary allocations may exceed retained steady state. Record hardware, build ID, project count, versions and measurements in the implementation record before claiming the budget is met. No latency/memory result exists yet, and FTS has not been justified by measurement.

## VPS deployment tooling (Epic 06)

See [hosting.md](hosting.md) for setup, prerequisites, the shared-resource registry, Cloudflare settings and staged cutover. Scripts run locally from the Mac over key-only SSH as `pixlie` with passwordless sudo. Shared non-secret settings live in `deploy/wrkgn.conf`.

```sh
scripts/setup-vps.sh --credentials ~/.cloudflared/<tunnel-uuid>.json
scripts/deploy.sh
scripts/publish-data.sh
scripts/status.sh
scripts/deploy.sh --rollback
scripts/publish-data.sh --rollback
scripts/publish-data.sh --purge-only
```

Deploy requires Node, Python 3, Zig, cargo-zigbuild and the installed Rust Linux target. It detects VPS architecture, Debian and glibc, targets glibc 2.31 (rejecting older hosts), and explicitly selects LLVM release codegen. The native checker remains at `target/release/wrkgn-web` for subsequent publishing. Use `--export PATH` on deploy/publish for another export; publish also accepts `--checker PATH` or `WRKGN_WEB_CHECK_BIN`. First deploy seeds a validated snapshot only if none is selected on the server. Later deploys preserve server data. Deploy does not purge unless `--purge` is supplied. `--allow-dirty` permits and labels an uncommitted build.

`CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ZONE_ID` are read locally from environment or `.env`; only those assignments are parsed without executing the file. Purges name both configured hostnames. Missing token warns and skips purge; a failed purge after a successful publication requires `--purge-only`. Tokens never go to the VPS. A shared server lock serializes release and snapshot pointer changes.

For shell changes, run `shellcheck scripts/*.sh scripts/lib/*.sh` when available and `bash -n` on each script. Agents ran Bash syntax checks only; ShellCheck was unavailable. Live setup, deployment, publication, rollback, cache and browser checks were later run by the maintainer; see [the implementation record](implementation/epic06.md). Keep Astro and JSON compatibility until the documented cleanup gate.

## Static website (Epic 03)

The Astro site lives in `website/`. It reads only `website/data/public/`, which is a selected, tracked public snapshot. The staged snapshot contains approved pilot records. Synthetic fixtures in `website/tests/fixtures/representative/` and `website/tests/fixtures/empty/` support offline UI work and must not be presented as real HN content.

After reviewing and approving mentions locally, run `cargo run -- export` from the repository root. Then explicitly stage the completed export and build from `website/`:

```sh
cd website
npm ci
npm run stage -- ../local-data/public-export
npm run dev
npm run build
npm test
```

`npm run stage` takes one export directory containing `current.json`. It validates public schema version 6 (and accepts existing version 2, 3, 4, and 5 snapshots for compatibility), the selected manifest (including the category and audience taxonomies for versions 3 through 6), every discovery record, every referenced project detail, and the public field allowlist. It copies only those files into a replacement staged snapshot; previous staged versions are removed. A missing or invalid export fails before replacing the staged data. `npm run dev` and `npm run build` consume the staged snapshot without running Rust, contacting HN or a model, or reading local credentials or SQLite. Static output goes to `website/dist/`; the versioned browser data is under `/data/<version>/`.

The retained Pages fallback uses `website/` as the build root, `npm run build` as the command, and `dist/` as output. Do not push large snapshots to main while Pages is connected: builds fail above 20,000 files. A real correction and removal contact destination must be supplied before launch; do not publish a placeholder route.

The directory renders ordinary list and project links without JavaScript. After discovery data loads, search and Category and Audience appear in the shared fixed left sidebar on wide screens. The first 30-project page is `/`, with `/projects/` as an alias; later pages use `/projects/page/N/`. Direct detail pages offer an ordinary filter form that returns to `/`. On narrow screens (below 768px), a Filters button in the primary navigation opens the same controls in an opaque, fixed full-screen overlay. Its contents scroll independently above a pinned Clear all / Close footer, while the underlying page is locked. Close or Escape restores page scroll and focus. Resizing to desktop closes the overlay and restores the sidebar; Close is hidden on desktop. On listing pages, the button and controls stay hidden when discovery data cannot load; detail-page controls remain available as a normal form. Search matches the exported `searchable_text` for every published project. Selected filter values are OR within one dimension and AND across dimensions and the text query. The Sort by dropdown on the right of the count row offers comment count in both directions and date in both directions, defaulting to recent first. Sorting applies to all matches before pagination, with newest date then project ID resolving equal counts and project ID resolving equal dates. A sort change resets page one; Clear all also resets the default order. Legacy snapshots without counts offer date orders only. Criteria, sort, and interactive page state use URL parameters; invalid values are dropped and out-of-range pages clamp to the last available page. Opening a card fetches versioned detail JSON and the matching standalone page HTML, then displays its shared detail markup in a modal. A failed detail fetch navigates to the ordinary project page.

### Manual browser checklist

The maintainer performs browser interactions; no browser automation is required. For Rust use a schema-6 export and the server command above. For Astro compatibility stage the same export and build separately. Synthetic fixtures must never replace approved production data inadvertently.

- Test `/`, `/projects/` (Rust 301 to `/`), later `/projects/page/N/` pages, ordinary pagination links and direct `/projects/<slug>/` pages. Unknown slugs return 404. Disable JavaScript and check search, both filters, sorting, pagination, cards, and detail controls; Rust controls remain visible and submit GET forms.
- Search for builder usernames, Unicode, one/two-character terms, missing terms, and inner whitespace. Try all four sort orders, combined filters, multiple values within a filter, later pages, and Clear all. Reload, Back and Forward restore state. Invalid/default/duplicate parameters canonicalize with 301; absent options and out-of-range pages use 302. Queries are trimmed and capped at 100 characters. Check result counts and empty states.
- With JavaScript enabled, search updates after debounce and filters/sort update immediately without a full navigation. The first search update adds history and further typing replaces it. Filters/sort/page changes each add an entry. Failed fetches navigate normally.
- Open a card modal, inspect source dates/authors/evidence, close via button, Escape and Back, then Forward to reopen. Closing restores scroll/focus and list state. Modified clicks and refresh use standalone pages. A failed HTML fetch or snapshot mismatch falls back to standalone navigation.
- At wide widths, inspect the fixed sidebar, typography, card grid and compact toolbar. At narrow widths test the opaque full-screen Filters panel: keyboard focus trap, scroll lock, independent content scrolling, pinned Clear all/Close, Escape, focus restoration, and resize reset. Test with JavaScript disabled too.
- Inspect title evidence, dated mentions, actual evidence authors, builder convention, classifications, same-builder links, source links and independence notice. Check CSP console output; assets must require no inline-code exceptions. A real correction/removal contact remains a launch prerequisite.

Astro retains its old browser-data behavior until the stable-period cleanup: listing controls appear after discovery JSON loads, `/projects/` remains an equivalent list, and modal details use JSON plus page HTML. These are transitional differences from the Rust server.

## Project extraction schema migration

Database migration 6 stores private author and normalized URL aliases for project identity. It seeds aliases from existing mention links on first open and records them before replacement, so a successful zero-mention rerun does not erase a reviewed match or suppression. It does not modify existing project IDs, slugs, review decisions, or staged exports. Existing duplicates require source inspection and explicit `review match`; opening the database does not reconcile them automatically.

Distinct named projects in one monthly source may share a URL. Colliding mention keys use a deterministic name suffix; a uniquely matching name within that same source can retain its previous key and project, preserving corrections and suppression when output order changes. Exact URL-derived mention keys remain stable when a display name or additional links change. Synthetic `:sole` keys require a matching name when a prior shared-URL split makes the surviving project's identity uncertain. This does not enable name-based matching across sources. Shared-URL collisions do not automatically merge projects. Indistinguishable mentions or unresolved reviewed identities fail without replacing prior results; ambiguous suppressed URL matches still fail closed.

Database migration 5 adds source excerpts and additional builder records without deleting legacy request/supporting history. Public schema 3 omits requests, supporting contributions, and Looking for, and adds audience taxonomy metadata, source excerpts, and per-mention builder usernames. Missing legacy source excerpts or unknown legacy audience labels cause export to stop with instructions to reprocess; labels and approvals are not silently rewritten. Reprocess and review before staging a new export. Existing staged schema-2 data remains readable by the website.


## Show HN storage and public evidence

Database migration 7 adds a persisted extraction mode per processed thread (`monthly` or `show_hn`). Existing processing runs are backfilled as monthly without changing their fingerprints, prompt/schema versions, approvals, identities, or slugs. Collected threads with no processing history remain unassigned. Mode assignment rejects conflicting interpretations; collection alone never chooses a mode.

`mention_evidence` stores each supporting source ID, source field (`title`, `url`, or `body`), verbatim value, supported fields, source fingerprint, and optional submitter-authorization dependency and fingerprint. Existing monthly records retain their single-comment excerpt without invented field-level support. Show HN chunk results are cached privately in `show_chunk_cache`. Each chunk key includes its source and ancestor state, repeated submitter context, suppression, model, and processing versions; consolidation inherits its inputs' dependencies. Every consolidation level receives the actual authors, original timestamps, and parent IDs for its evidence and authorization sources, plus the story submitter identity; it does not receive whole source bodies. The complete prompt including metadata must fit the input bound. Explicit submitter corrections take precedence only when their chronology and meaning are clear; ambiguous disputes are omitted. Independent reply edits or appended branches reuse unaffected chunks, while the final result retains a whole-thread freshness identity. `show_processing_results` retains a successful no-project reason. These tables are local processing state and never website assets.

New public snapshots use schema 6. Each mention retains its anchor author, original sharing date, source URL, and legacy excerpt field, and adds an `evidence` array. Each evidence entry contains `source_item_id`, `source_field`, `value`, `supports`, `author`, `shared_at`, and `source_url`. Evidence authors and dates come from the actual source item; supporting replies do not change the story's sharing date. Legacy monthly evidence uses `body` and an empty `supports` array. The public array exposes concise excerpts rather than full discussions. Source and authorization edits, confirmed removals, deletion, or suppression conservatively withhold the affected mention until valid processing replaces it. Private authorization fingerprints and processing diagnostics remain excluded.

The website explicitly supports public versions 2, 3, 4, 5, and 6, rejecting unsupported versions. Staging, standalone pages, and modal readers use the same validated contract; schema-5 details render each evidence field with its actual author, date, and HN source link. Title evidence is presented as a title rather than a manufactured comment excerpt. Builds and staging make no live HN or model calls.
