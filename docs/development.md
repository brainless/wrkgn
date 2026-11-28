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

The storage library opens `local-data/wrkgn.sqlite` by default. Set `WRKGN_DB_PATH` to a different SQLite file when needed. Its parent directory is created automatically. The working database belongs outside any directory that is served or published; the default `local-data/` path and `.env` files are ignored by Git.

SQL migrations in `migrations/` are applied transactionally using SQLite's `user_version`. Schema version 8 adds the run, coverage, queue and run-error tables (`driver_runs`, `coverage_units`, `run_queue`, `run_errors`; `migrations/008_runs.sql`); version 9 adds `coverage_units.acknowledged_at` and `acknowledged_note` (`migrations/009_acknowledged_units.sql`). The source tables retain raw HN payloads, item IDs, authors, original timestamps, parent IDs, sibling order, deleted/dead flags, and first/last fetch times. Each changed source snapshot gets a numbered revision; a repeat fetch with identical source data only advances its last fetch time. Content fingerprints cover the body, title, URL, author, original timestamp, and deleted/dead state, while child-list and score changes are excluded. The revision history still records raw payload changes.

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

`--db <path>` selects the local SQLite database. `--export <directory>` selects the public export directory; `WRKGN_EXPORT_PATH` overrides the default `local-data/public-export`. `process` never fetches from HN: it reads stored source and requires the latest collection run for the thread to be `complete`, otherwise it fails and asks you to rerun `collect`. Run `collect` first, then `process`. `--concurrency <n>` (default 10) sets how many conversation groups are extracted in parallel; it is the maximum, and a shared adaptive limiter lowers the number of in-flight model calls when the provider reports a concurrency limit (for example `5/5 slots in use`), then restores it by one after five consecutive successes. Concurrency-limit errors retry after a short jittered pause (the provider's suggested delay, at most 3s) without consuming attempts, up to 30 such retries per call before the call counts as exhausted; the same applies to Show HN extraction. Other rate-limit (429), provider 5xx, and connection errors are retried up to six attempts with jittered exponential backoff from 1s to 30s. After three groups fail with such errors (cumulative across the run), no new groups are started; already claimed groups finish, with further retries disabled after the stop signal; the rest are reported as not attempted, and rerunning without `--reprocess` resumes, skipping groups completed with the current prompt, schema, and model. Progress (`Extracting n/total groups (pct%), k failed`) goes to stderr, in place on a terminal and at 10% steps otherwise. On failure the CLI prints counts by error kind with one example group each; full errors are in the `processing_failures` table for the reported run. Results are written to SQLite on one thread in group order, so a group's result commits as soon as it and every earlier group are done. Unchanged, successful groups are skipped unless `--reprocess` is supplied. Prompt, model, schema, and source fingerprints determine whether a group is unchanged. A reply edit invalidates its top-level group. If one group fails, successful groups still commit, the failed group's previous result remains in SQLite, the processing run is marked incomplete, and the standalone `process` command exits nonzero. Oversized items or ancestor chains produce a recorded failure rather than truncated input.

Rig extraction uses a process-wide Tokio runtime that remains driven for the lifetime of its shared HTTP connection pool. A processing run coordinates retry cooldowns across its workers so that new attempts wait for the shared backoff deadline after transient failures; already running requests may finish. Rig internal retries are disabled so provider attempts pass through the shared cooldown. Transport diagnostics retain bounded error causes with the configured credential and endpoint redacted.

HN may reorder top-level comments between refreshes. A group's rank among other top-level comments does not change its extraction input and does not trigger a model call. Compatibility checks compare existing cache hashes against that root's previously collected positions, requiring the entire current subtree to otherwise match, then advance its fingerprint for publication without changing approvals. Existing cache hashes, prompt/schema versions, and model identities remain supported. Edits, additions, removals, or ordering changes within the reply subtree still invalidate the group. The collection report's changed-item count includes sibling-position and raw-payload changes; it is not a count of edited comments or required model calls.

Each model invocation receives one top-level comment subtree, or bounded chunks with root and ancestor context. All source IDs must be accounted for. HN HTML is converted to plain text with full link destinations before extraction and validation. Structured JSON output contains project mentions with a verbatim `source_excerpt`, a project-centric description, optional problem/audience text, category and audience slugs, and additional explicitly named `builders`. The source author is assumed to be a builder; `builders` excludes that author. Missing names may become short descriptive names. Replies provide context or introduce their own projects with their actual source IDs; requests and separate supporting contributions are no longer extracted.

Structured-output deserialization and local validation failures share up to two correction attempts per batch, including Show HN extraction, whose correction prompt restates the one-object response shape (`Options::max_validation_corrections`, default 2, capped at 2). Correction prompts retain the original source records and carry validation feedback and, when available, a bounded rejected response as untrusted JSON data. Every corrected response undergoes the same schema and provenance validation before any group results are saved. Exhausted validation errors record the source ID and bounded rejected field when available in the private `processing_failures.error` column; URL diagnostics omit credentials, queries, and fragments. Validation failures remain separate from provider failures even when rejected text contains transport-error wording. Retry-policy changes do not invalidate successful groups or require renewed approval by themselves.

Categories and audiences are proposed classifications. Audience slugs come from [audiences.md](audiences.md), with zero to five distinct values only when the source mentions the audience. The prompt and schema versions change so existing groups are processed again on the next explicit process run. Existing approved records are not silently reclassified or approved by a database migration. New extraction through the manual CLI still requires review; `wrkgn-monthly` automatically approves after each successful thread. Monthly extraction first normalizes each mention's URLs: whitespace anywhere in a URL is removed, a scheme-less value gets `https://`, and an uppercase scheme is lowercased; duplicates collapse. A URL still not an http(s) URL with a dotted host (for example `ftp://x` or `localhost`) is dropped from that mention, since URLs are optional, and the group does not fail. The remaining URLs and excerpts are validated against the supplied source, including URLs attached to labels, footnotes, and punctuation wrappers. Source matching ignores scheme (http/https/none), a leading `www.`, host case and a trailing slash, so a source `coolweird.net` supports `https://www.coolweird.net/`. A scheme immediately followed by backslashes (`https:\\host`, `HTTP:\host`), as an author may type, is read as `http(s)://` in both source and model output; backslashes elsewhere are untouched. URLs are checked only against the mention's own source item, never its replies; paths, queries and fragments must still match, and a URL absent from the source in every form still fails validation and is corrected or the group fails. Safe project-origin normalization is permitted for multiple same-origin demo URLs. These checks are validation only: prompt and schema versions are unchanged. Shared-hosting project paths must be preserved, and URLs embedded inside another URL do not count as separate source links.

New mentions are pending review. `review pending` prints one readable card per mention: extracted fields with corrections applied, labels, builders, links, and source evidence. Show HN cards display every stored evidence quotation with its source field, actual author, original date, HN source link, and supported fields. Authorized-builder evidence links to the submitter authorization source; its identifying quotation is also displayed among the evidence. Monthly cards retain the single-comment excerpt and context without invented field-level support. It ends with the pending IDs as compact ranges. Set `COLUMNS` to change the card width; `--json` retains the existing one-object-per-line fields, applies corrections, and adds an `evidence` array (empty for legacy monthly mentions) with source metadata and optional authorization source IDs. Review output does not include private fingerprints, raw payloads, or processing diagnostics. `approve` takes IDs and inclusive ranges (`12-30 34`) and approves them in one transaction. Non-pending IDs inside a range are skipped. A range with no pending mentions fails without approving anything. Review the cards before approving a range; corrections persist by mention key across reruns. Successful reprocessing returns changed material to pending review. Source and project suppression survive reruns. There is no command to undo suppression yet; inspect and edit local SQLite deliberately if a suppression was erroneous.

## Automated monthly binary

`cargo run --bin wrkgn-monthly --` reuses `Collector`, `process_thread_with`, thread-scoped approval and the standard public exporter, driven by the shared run driver in `src/run/` (see below). With no arguments it resumes unfinished months and threads, re-walks the head (current and previous UTC months, `--months`, default 2) completely, backfills the newest uncovered older months (`--backfill-months`, default 1, floor 2007-02), and stops. `--max-items` (default 5) caps threads taken from resume and backfill; head threads are not counted, so backfill advances every run. Months of threads already stored are seeded as covered from their titles on first use. `--from YYYY-MM` and/or `--through YYYY-MM` override this: they process exactly that inclusive range, newest first (`--from` alone covers 24 months ending there; `--through` alone runs from the current month back), bypass head, backfill and limits, and cannot be combined with `--months`, `--backfill-months` or `--max-items`. The existing `--db`, `--export` and `--concurrency` settings apply. `default-run = "wrkgn"` keeps existing `cargo run -- ...` commands unchanged.

Discovery queries the HN Algolia story-title index separately for each full and abbreviated month name plus year, pages through all hits, filters whole titles locally, and deduplicates IDs. It accepts three-letter abbreviations (also `Sept`), optional `Ask HN:`, and question marks. Query word removal and prefix matching are disabled. An estimated hit count (`exhaustiveNbHits=false`) is accepted; counts exceeding the available pagination window or unexpectedly empty pages fail explicitly instead of silently dropping results. Network errors and 429/5xx responses receive up to three attempts. Indexing delays can hide new discussions until a later explicit rerun, because a month with no matches is recorded done. All matching threads in a month are handled before the previous month.

The collector refreshes the full tree so new replies, edits, and deletions retain existing semantics. The processor skips unchanged successful groups using its existing source/prompt/schema/model checks. After a complete successful thread pass, its pending mentions are automatically approved, including earlier pending results; other threads are unaffected. Corrections, matching, and suppression persist. Export remains cumulative across all approved threads. A thread failure does not stop the run; failures are collected into a grouped end report, and a month discovery failure fails only that month. Three consecutive exhausted calls to one API stop the run (breaker, below). A partial processing pass attempts a freshness-filtered export but does not auto-approve the thread. Approval and export commit separately, so rerun after an export failure. Setup/configuration, database-open and output-write failures remain fatal. The workflow never stages website assets. See [the maintainer runbook](../MANAGE.md#automated-monthly-workflow).

## Automated Show HN binary

`wrkgn-show-hn` with no `--thread` or range uses the same driver with UTC day units: resume, head (`--days`, default 3), backfill (`--backfill-days`, default 3, floor 2007-02-01), capped at `--max-items` (default 100) stories from resume and backfill. A day is discovered in Algolia batches (`--batch-size`) with the inclusive timestamp cursor; each batch is collected, extracted and exported once. There is no seed from earlier data: days processed before coverage existed may be collected again, and unchanged extraction is skipped. `--from` and `--through` (both required) re-walk exactly those days and bypass head, backfill and limits. `--thread` is item-level: it changes no day coverage and exits 1 on failure. Results stay pending unless `--auto-approve`.

## Shared run driver

`src/run/` (`mod`, `driver`, `control`, `lock`, `signals`, `errors`, `status`, `report`) implements everything common to the two commands; the `Workload` trait leaves discovery as the only per-command part.

- Persistence: migration 008 tables `driver_runs`, `coverage_units`, `run_queue` and `run_errors`. A queue entry is keyed by command, kind and item; the monthly thread-to-month mapping is held in the queue. Empty units are marked done and not searched again. A unit failure is retried on later runs up to `MAX_UNIT_ATTEMPTS` (3), then listed for manual action. `wrkgn runs list|acknowledge|unacknowledge|retry --command monthly|show-hn [--unit <unit>] [--note <text>]` resolves them locally without network calls (mutations refuse while the command's run lock is held): `acknowledge` skips a failed unit and removes it from failures and the exit code while the report keeps an `Acknowledged (skipped)` line, `unacknowledge` reverses it, `retry` resets attempts and failed queue items for the next default run. A default run also reopens an exhausted unit whose failed threads now have a complete latest collection and a complete later processing run (a manual `wrkgn collect --thread` then `wrkgn process --thread`), so it is re-collected, approved and exported without new model calls.
- Lock: `<db>.<command>.lock` holds the PID. A second live run exits with code 1; a stale lock is taken over and the old run recorded as `killed_detected`.
- Signals: SIGINT/SIGTERM set a stop flag checked at item boundaries; the run exports, reports and exits 130. A second Ctrl-C exits immediately. The `Collector` honours the flag: a stopped collection ends incomplete (`finish_run_with`) and is retried later.
- Breaker: three consecutive exhausted calls to one API (LLM, HN, Algolia) stop the whole run with exit code 2. Only a call whose retries are exhausted counts; validation failures, null items and empty results do not, and any success resets the count. `wrkgn process` builds a per-call breaker with the same threshold, so a thread's processing stops after three consecutive exhausted model calls.
- Report: live status and a Markdown end report go to stderr, with errors grouped by normalized signature and full samples, work done and what remains. No log files exist. Credentials, raw payloads and prompts are excluded.
- Exit codes: 0 completed or capped without failures; 1 failures or setup error; 2 breaker; 130 interrupted. Exhausted units from earlier runs keep exit 1 until resolved (retried, fixed by hand or acknowledged), so the report header explains a nonzero code (`Exit code: 1 (4 units need manual action, all from earlier runs; 0 errors this run)`) and separates units exhausted this run from carried-over ones. A `limit_reached` outcome names its condition: item cap (`thread cap reached (5/5)`), remaining backfill (`more backfill remains (next: 2022-02)`), or both. Remaining backfill is normal for years and is not a problem; the stored status stays `limit_reached`.

Offline tests inject paginated discovery responses, HN fixtures, and a deterministic model; shared fakes live in `tests/common/` and the driver is covered by `tests/run_lock.rs`, `tests/run_breaker.rs`, `tests/run_report.rs`, `tests/run_monthly.rs` and `tests/run_show_hn.rs`. Ordinary checks never call live HN or LLM services.

## Public snapshot contract

Schema 6 snapshots consist of `public.sqlite` plus a small `manifest.json`
(`schema_version`, `version`, `database: "public.sqlite"` and `project_count`)
in each completed `snapshots/<version>/` directory. `current.json` has three
fields (`schema_version`, `version`, `snapshot`) and points to that manifest.
The version digest begins with `public-schema-6` and hashes the public records
and taxonomy metadata, never the SQLite bytes or export time.

Removing the former JSON files (`discovery.json`, `projects/<slug>.json`) did not
change the public schema, so it stayed at 6. Consumers (`wrkgn-public-snapshot::select`,
`wrkgn-web`, `scripts/deploy.sh`, `scripts/publish-data.sh`) read only the pointer,
manifest `schema_version`/`version`/`database` and `public.sqlite`; the database
schema, the digest input and its prefix are unchanged, so existing versions
remain stable and the server needs no migration. Bump the schema only when the
database layout or the pointer/manifest fields consumers rely on change.

The `wrkgn-public-snapshot` workspace crate owns public record types, schema and
validation. SQLite has `metadata`, `taxonomy`, `projects`, `project_builders`,
`project_labels`, `mentions`, `mention_builders`, `mention_links`, and `evidence`
tables, with indexes for discovery order, builder lookups, labels and mentions.
Project IDs/slugs and all displayed mention/evidence attribution are retained.
Evidence support mappings, authorization dependencies, private mention keys,
review/processing history and raw payloads are excluded. Metadata holds schema
version, snapshot version and export time; taxonomy rows hold slugs, labels and
definitions. The database is built from the captured public model,
written to an unpublished temporary file, vacuumed and validated before the
snapshot directory and pointer are renamed. Validation checks the exact object
and column allowlists, required indexes, integrity and foreign keys, metadata,
fixed taxonomy slugs, unique identities, safe slugs, HTTP(S) links, HN source
identities and required dated evidence. Read-only consumers reject journal/WAL
sidecars and validate the selected version before use.

`export` reads all approved projects from one SQLite transaction, applies corrections and suppression, and creates `snapshots/<version>/manifest.json` and `public.sqlite` under the export directory. `current.json` is replaced atomically and points to the completed snapshot. The version hashes both discovery and detail data. Consumers read `current.json`, then its manifest and database; nothing copies the working database or raw payloads. Export only adds versions and never deletes any, so `snapshots/` grows without bound; interrupted exports can also leave `.staging-<pid>-<version>` directories. Local copies other than the one `current.json` selects are not read by any script: `scripts/publish-data.sh` and `scripts/deploy.sh` resolve only `current.json`, `wrkgn-web` loads only the pointed snapshot, and both rollbacks (`publish-data.sh --rollback`, `deploy.sh --rollback`) switch the server's own `previous` links without any local input. `wrkgn snapshots prune [--keep N] [--dry-run] [--export DIR]` (`src/snapshots.rs`) is the explicit, offline, database-free cleanup: it always keeps the `current.json` version plus the `--keep` (default 2) most recently modified others, removes the rest and `.staging-*` directories whose owning PID no longer runs, and reports the apparent bytes reclaimed. It refuses when `current.json` or its snapshot is invalid, when a staging directory's process is alive, and never follows symlinks or touches anything but direct children of `<export>/snapshots` (unrecognized entries are skipped and reported). Run it only while no export is running; export and `process` never invoke it.

Discovery records have stable `id` and `slug`, card fields (`name`, `description`, `problem`, `audience`), `builder_usernames`, `categories`, `audience_labels`, `latest_shared_at`, `comment_count`, and `searchable_text`. They are ordered by latest original mention date, then project ID. Public schema 6 retains `comment_count` from version 4: distinct replies beneath a project’s published source comments, including nested replies, excluding deleted, dead, absent, and source-suppressed replies. Overlapping subtrees are deduplicated. Counts use stored source at export time; no HN or model request is needed. Comment vote counts are unavailable from HN. Detail records have `id`, `slug`, `name`, and dated `mentions`; each mention includes its source ID and URL, author, links, verbatim source excerpt, and builder usernames. Dates are Unix seconds from HN source records. A current source edit, deletion, missing item, or suppression hides affected content; a failed rerun does not republish stale group results. Rust HTML and modal HTML use this snapshot.

The maintainer has inspected many detail pages and the project list and considers extraction quality sufficient to offer the explicit automated monthly workflow. Schema validity alone does not establish extraction accuracy; manually inspect descriptions, labels, invitations, provenance, and grouping before publication.

## Rust web server (Epic 05)

`crates/wrkgn-web/` depends on `crates/public-snapshot/`, with no pipeline/Rig/HN dependency. It renders Askama templates from a read-only public SQLite pool on blocking threads. Discovery/search fields live in memory with four presorted ID orders. It serves HTML rather than discovery/detail JSON. Epic 06 provides separate VPS deployment scripts; these local commands never deploy. Production has served from the VPS since the 2026-11-18 cutover.

From the repository root:

```sh
npm ci --prefix crates/wrkgn-web/assets
npm run build --prefix crates/wrkgn-web/assets
cargo run -p wrkgn-web -- check local-data/public-export
cargo run -p wrkgn-web -- --snapshot local-data/public-export
```

The server defaults to http://127.0.0.1:3000. `check` accepts a snapshot root or a single `public.sqlite` and performs startup validation offline. Existing schema-5 exports need a deliberate `cargo run --bin wrkgn -- export`; a model rerun is unnecessary solely to generate schema 6. The working database is never opened by the server.

Assets live in `crates/wrkgn-web/assets/`, compile with esbuild to ignored `dist/`, and embed into the binary at content-hashed paths. `build.mjs` writes a flat `manifest.json` (key to filename) last; `build.rs` embeds every entry, requires keys matching `[a-z][a-z0-9_]*`, exposes each as a `<KEY>_URL` constant in `render` (`SCRIPT_URL`, `STYLE_URL`, `ICON_URL`, and `OG_IMAGE_URL` when `assets/src/og-image.png` exists; it does) and derives the MIME type from a fixed extension table (js, css, svg, png, woff2; anything else fails the build; see `asset_manifest.rs`). `npm run watch --prefix crates/wrkgn-web/assets` rebuilds after source edits; restart Cargo after a rebuild to embed changed assets. Build assets before every Rust build/check/test sequence. Missing assets produce an explicit Cargo error. Node is needed only during maintainer builds. Fonts are self-hosted: `build.mjs` copies and hashes the Latin and latin-ext, normal-style, weight-axis-only WOFF2 files of `@fontsource-variable/dm-sans` and `@fontsource-variable/source-serif-4` (exact versions pinned in `package.json`, covering weights 400-700) into `dist/` as `font_dm_sans`, `font_source_serif_4` and their `_latin_ext` counterparts, prepends one hashed `@font-face` rule per file (with the `unicode-range` read from the package CSS, so latin-ext downloads only when needed; `font-display: swap`, same family names and CSS fallbacks) to the CSS bundle before hashing it, and `layout.html` preloads only the two Latin files with `crossorigin`. Their OFL license texts are kept in `crates/wrkgn-web/assets/fonts/LICENSE-*.txt`, not embedded or served. Characters outside both subsets fall back to the CSS fallback fonts.

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

The public origin must be an HTTP(S) origin without credentials, path, query or fragment. Pages load no external styles, fonts or scripts, and the CSP allows only `'self'` for styles and fonts. Responses send CSP, nosniff and strict-origin-when-cross-origin; scripts/styles need no inline exceptions. Compression is gzip; `Vary` contains only `Accept-Encoding`, and responses never set cookies. Cacheable representations have snapshot/build/canonical-URL ETags and support weak `If-None-Match`. Saturated list rendering returns 503 with Retry-After and no-store.

Routes preserve slashed page URLs. `/projects/` and `/projects/page/1/` redirect to `/`; unslashed pages redirect with query preserved. GET/HEAD are supported; other methods return 405. Legacy `/data/...` is 404. `/robots.txt` disallows query-string discovery URLs. `/sitemap.xml` lists, in fixed order, list pages, then `/categories/` and page 1 of each category with projects (alphabetical by slug), then project details, splitting into `/sitemap-N.xml` parts at 50,000 URLs (the split counts all three groups; `server::sitemap` takes the limit so a test can exercise the boundary). Page 2 and later of a landing page are indexable and self-canonical but not listed. Project entries carry `<lastmod>` (W3C, UTC) from `latest_shared_at`, which is the latest sharing date and only a freshness hint, not a page modification time; list, landing and index pages have none, and there is no `changefreq` or `priority`. Criteria pages (`/?…`) are noindex and canonicalize to the unfiltered list; that includes `/?category=<x>`, which deliberately does not redirect or canonicalize to `/categories/<x>/` (`robots.txt` already disallows `/?`).

### Category landing pages

`/categories/` lists each category that has projects (label, taxonomy description, project count, link). `/categories/<slug>/` and `/categories/<slug>/page/N/` show the label as `<h1>`, the taxonomy description as intro, the project count and the same 30-per-page cards and pagination, newest first. Non-canonical forms redirect like list routes: missing trailing slash (301, query kept), `page/1/`, leading zeros, `?page=N`, and any query string (tracking parameters, `q`, `category`, `sort`) 301 to the clean landing URL; a page past the end 302s to the last page. Unknown labels and labels with no projects return 404. Landing and index pages use the Public cache class and share the render semaphore (503 with `Retry-After` when saturated). Titles are `<Label> projects · wrkgn` (page N: `<Label> projects – Page N · wrkgn`); the description is the taxonomy description cut to 155 characters (page N appends ` Page N.`); they carry a self-canonical URL, no robots meta (index, follow) on every page, Open Graph/Twitter tags and a minimal `CollectionPage` JSON-LD block. Counts come from in-memory discovery data; the snapshot schema is unchanged.

URL model: `State` has an optional `fixed` criterion (`Fixed { facet, slug }`) read from the path. It is not a query criterion, so `has_criteria()` stays false, `state_url` returns the landing path (plus `page/N/`), and `read_state` ignores query filters, search and sort on landing paths. `resolve` returns `not_found` for an unknown label. The mode is mirrored in `assets/src/canonical.ts` and the shared `tests/fixtures/canonical-urls.json` (status 404 marks `not_found`). Audience pages use the same mode: `Facet::LANDING` lists both facets and `canonical.ts` `LANDING` maps `category` and `audience` to their index paths, so `/audiences/`, `/audiences/<slug>/` and `/audiences/<slug>/page/N/` share every route, redirect, 404, cache and render path with categories. Audience titles read `Projects for <Label> · wrkgn` (`Facet::page_name`); category titles stay `<Label> projects · wrkgn`.

Controls on a landing page: the sidebar form shows the landing's category or audience checked, so search, other filters and sort navigate to the equivalent `/?category=<slug>…` or `/?audience=<slug>…` query result (noindex), and Clear all goes to `/`. Pagination stays on the landing path and `data-list-url` is the landing URL. Category tags on cards and project pages, and audience tags on project pages (cards show no audiences), are `<a class="tag tag--link">` links to the landing page, placed beside (not inside) the card's `data-project-link`, so the modal never intercepts them; clicked inside the project modal they navigate fully. "Categories" and "Audiences" links appear in the sidebar, mobile header and footer. The sitemap lists list pages, then the category index and pages, then the audience index and pages, then projects.

### Page metadata

Head metadata is built in `render.rs` from a `Head` struct and the pure helpers in `src/metadata.rs`; all lengths count `chars()`. The homepage title is `Projects shared on Hacker News · wrkgn`; `/projects/page/N/` adds `Page N` to the title and description; search and filter results use `Search results` or `Filtered projects` (noindex). Project titles are `<name> – <short description> · wrkgn`, about 60 characters: a case-insensitive name prefix (ending at a word boundary) and following separators are stripped from the description, the remainder is cut at a word boundary with an ellipsis, and the name alone is used when nothing useful fits, including when a truncated fragment would be shorter than 20 characters before the ellipsis (`MIN_FRAGMENT`; a short description that fits whole is kept). Unnamed projects use the `Unnamed project` fallback without prefix stripping. The `<h1>` and visible text are unchanged. The meta, `og:` and `twitter:` descriptions are truncated to about 155 characters at a word boundary with an ellipsis; visible descriptions stay full. Every indexable page and noindex list page emits `og:site_name`, `og:type` (`website`, or `article` for projects), `og:title`, `og:description`, `og:url` (the absolute canonical URL) and `twitter:card`, `twitter:title`, `twitter:description`. Pages with these tags also emit `og:image` (the maintainer-approved 1200×630 `assets/src/og-image.png`, hashed as `OG_IMAGE_URL` and made absolute with `--public-origin`), `og:image:type`, `og:image:width` 1200, `og:image:height` 630, `og:image:alt`, `twitter:image`, and `twitter:card` is `summary_large_image`. The PNG is generated on macOS by `assets/tools/og-image.swift` (`swift og-image.swift <out.png>`, Helvetica Neue Bold) and checked in; no build runs the script. Error pages keep a minimal head with none of these tags.

JSON-LD is one `application/ld+json` block, serialized with `serde_json` with every `<` escaped. The homepage emits `WebSite` with `name` and `url` only (no `SearchAction`). Project pages emit `CreativeWork` with the full visible description, canonical `url`, `datePublished` as the earliest `shared_at` over the detail's mentions, and one `Person` author per listed builder (name is the HN username, url `https://news.ycombinator.com/user?id=<percent-encoded>`; a listing convention, not verified ownership). There is no `dateModified`. Landing and index pages emit a minimal `CollectionPage` (name, description, url). Other list pages emit no JSON-LD. After in-page (modal or list) navigation `syncHead` updates only title, canonical, robots and description, so Open Graph tags and JSON-LD can go stale until a full load; this is accepted because crawlers and preview bots fetch full pages.

Robots review: `robots.txt` keeps `Disallow: /?`, `/projects/?` and `/projects/page/*?`. Crawlers therefore never fetch filter or search URLs and never see their `noindex`; that is intended, since those URLs are low-value duplicates. Tracking parameters on the homepage (`/?utm_source=x`) 301 to `/`, and project URLs with extra query parameters return 200 with a canonical link to the clean URL and are not blocked. No concrete problem (for example Search Console reporting indexed pages blocked by robots.txt) has been found, so the rules are unchanged; revisit by blocking only filter/search routes if one appears.

Canonicalization runs syntax-only 301 rules first, then snapshot-dependent 302 rules. Search uses Unicode lowercase substring matching and a trimmed 100-character cap; inner whitespace remains significant. OR within filters, AND across dimensions/query, 30 results per page, with the existing four sort tie-breakers. Unknown published options and out-of-range pages use temporary redirects. Rust and TypeScript consume `crates/wrkgn-web/tests/fixtures/canonical-urls.json` in their respective test definitions.

### Related projects

Project pages (and the modal, which reuses the same HTML) show up to six "Related projects" in the aside, separate from "Also shared by this builder". The data is named `similar_projects` (`crates/wrkgn-web/src/similar.rs`, `SimilarIndex`) and is built once in `Snapshot::load`, so a SIGHUP reload rebuilds it for the new snapshot; requests only look up compact `u32` indices into `DiscoveryIndex::projects` (no per-request SQL or scoring). Scoring is deterministic for a snapshot:

- Primary score: sum over shared categories of `ln(N / df)` (N = projects, df = projects with that category), held as integer millionths so ties are exact. Secondary score: the same weighting over shared audiences. Final tiebreaks: `latest_shared_at` descending, then project id.
- Excluded: the project itself and any project sharing a builder username (the same set "Also shared by this builder" shows, computed from in-memory builder lists). Audience alone never qualifies, and projects without categories get no section; the section is omitted when nothing qualifies.
- Minimum: the primary score must reach `MIN_SCORE` (0.5) and exceed the weight of either of the two largest categories in the snapshot (found dynamically), so a match on only one of those two never qualifies.
- Construction uses per-category and per-audience inverted indexes and a bounded top-six insertion.

Measurement (release build, `memory_probe` against `local-data/public-export`, version 2e98be1d98451abc, 16,982 projects, Apple M4 with 16 GiB; three runs each, `ps` RSS, 4 pooled connections). The "before" runs temporarily disabled the precompute. `load_ms` covers `Snapshot::load` including validation:

| | load_ms | one snapshot RSS | two snapshots RSS (SIGHUP peak, approx.) |
|---|---|---|---|
| Without similar projects | 199-312 | 29,424-33,664 KiB | 56,224-59,280 KiB |
| With similar projects | 655-704 | 31,824-35,936 KiB | 59,504-61,072 KiB |

Precompute adds roughly 450 ms to load and about 2-3 MiB resident (resident cost per snapshot is the 17k-entry list of at most six `u32`s plus the offsets; transient scratch is freed). A SIGHUP reload briefly holds two snapshots, so the peak is about twice the single figure; this probe retains two full snapshots and does not measure reload polling peaks. These are local numbers, not VPS measurements.

### Maintainer performance commands

These tools are explicit local checks, not automatic startup/build behavior. They use only public SQLite and local HTTP. Run the TypeScript tests from the repository root:

```sh
npm test --prefix crates/wrkgn-web/assets
```

For performance, build optimized binaries and create a new public-only fixture directory; generation refuses to overwrite existing paths and copies taxonomy from the source export's `public.sqlite`. No working SQLite database is read:

```sh
npm run build --prefix crates/wrkgn-web/assets
cargo build --release -p wrkgn-web --examples --bin wrkgn-web
cargo run --release -p wrkgn-web --example synthetic_snapshot -- local-data/public-export /tmp/wrkgn-synthetic-100k 100000
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

For shell changes, run `shellcheck scripts/*.sh scripts/lib/*.sh` when available and `bash -n` on each script. Agents ran Bash syntax checks only; ShellCheck was unavailable. Live setup, deployment, publication, rollback, cache and browser checks were later run by the maintainer; see [the implementation record](implementation/epic06.md).

## Directory behavior

The directory renders ordinary list and project links without JavaScript. The first 30-project page is `/`, with `/projects/` redirecting to it; later pages use `/projects/page/N/`. Direct detail pages offer an ordinary filter form that returns to `/`. Search matches the exported `searchable_text` for every published project. Selected filter values are OR within one dimension and AND across dimensions and the text query. The Sort by dropdown offers comment count in both directions and date in both directions, defaulting to recent first; sorting applies to all matches before pagination, with newest date then project ID resolving equal counts and project ID resolving equal dates. Criteria, sort and page state use URL parameters. On narrow screens (below 768px) a Filters button opens the same controls in an opaque full-screen overlay with independently scrolling content and a pinned Clear all / Close footer; Close or Escape restores page scroll and focus. TypeScript enhances these ordinary HTML navigations and opens a card as a modal over the same project URL. A real correction and removal contact destination must be supplied before launch; do not publish a placeholder route.

### Manual browser checklist

The maintainer performs browser interactions; no browser automation is required. Use a schema-6 export and the server command above. Synthetic fixtures must never replace approved production data inadvertently.

- Test `/`, `/projects/` (301 to `/`), later `/projects/page/N/` pages, ordinary pagination links and direct `/projects/<slug>/` pages. Unknown slugs return 404. Disable JavaScript and check search, both filters, sorting, pagination, cards, and detail controls; controls remain visible and submit GET forms.
- Search for builder usernames, Unicode, one/two-character terms, missing terms, and inner whitespace. Try all four sort orders, combined filters, multiple values within a filter, later pages, and Clear all. Reload, Back and Forward restore state. Invalid/default/duplicate parameters canonicalize with 301; absent options and out-of-range pages use 302. Queries are trimmed and capped at 100 characters. Check result counts and empty states.
- With JavaScript enabled, search updates after debounce and filters/sort update immediately without a full navigation. The first search update adds history and further typing replaces it. Filters/sort/page changes each add an entry. Failed fetches navigate normally.
- Open a card modal, inspect source dates/authors/evidence, close via button, Escape and Back, then Forward to reopen. Closing restores scroll/focus and list state. Modified clicks and refresh use standalone pages. A failed HTML fetch or snapshot mismatch falls back to standalone navigation.
- At wide widths, inspect the fixed sidebar, typography, card grid and compact toolbar. At narrow widths test the opaque full-screen Filters panel: keyboard focus trap, scroll lock, independent content scrolling, pinned Clear all/Close, Escape, focus restoration, and resize reset. Test with JavaScript disabled too.
- Inspect title evidence, dated mentions, actual evidence authors, builder convention, classifications, same-builder links, source links and independence notice. Check CSP console output; assets must require no inline-code exceptions. A real correction/removal contact remains a launch prerequisite.

New public snapshots use schema 6. Each mention retains its anchor author, original sharing date, source URL, and legacy excerpt field, and adds an `evidence` array. Each public evidence entry contains `source_item_id`, `source_field`, `value`, `author`, `shared_at`, and `source_url`; support mappings stay private. Evidence authors and dates come from the actual source item; supporting replies do not change the story's sharing date. Legacy monthly evidence uses `body`. The public array exposes concise excerpts rather than full discussions. Source and authorization edits, confirmed removals, deletion, or suppression conservatively withhold the affected mention until valid processing replaces it. Private authorization fingerprints and processing diagnostics remain excluded.

The server accepts only public schema 6 and rejects other versions. Standalone pages and modal readers use the same validated contract; details render each evidence field with its actual author, date, and HN source link. Title evidence is presented as a title rather than a manufactured comment excerpt. Builds make no live HN or model calls.
