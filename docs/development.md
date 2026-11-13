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
cargo build -v
```

The verbose build command should include `-Z codegen-backend=cranelift` in the `rustc` invocation. If Cargo reports an unsupported `codegen-backend` profile key, check that the pinned nightly is active and the component is installed.

Run local checks with:

```sh
cargo fmt --all --check
cargo check
cargo test
cargo clippy --all-targets -- -D warnings
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

`--db <path>` selects the local SQLite database. `--export <directory>` selects the public export directory; `WRKGN_EXPORT_PATH` overrides the default `local-data/public-export`. `process` never fetches from HN: it reads stored source and requires the latest collection run for the thread to be `complete`, otherwise it fails and asks you to rerun `collect`. Run `collect` first, then `process`. `--concurrency <n>` (default 10) sets how many conversation groups are extracted in parallel; lower it if the provider rate-limits. Rate-limit (429), provider 5xx, and connection errors are retried up to six attempts with jittered exponential backoff from 1s to 30s. After three groups fail with such errors (cumulative across the run), no new groups are started; already claimed groups finish, with further retries disabled after the stop signal; the rest are reported as not attempted, and rerunning without `--reprocess` resumes, skipping groups completed with the current prompt, schema, and model. Progress (`Extracting n/total groups (pct%), k failed`) goes to stderr, in place on a terminal and at 10% steps otherwise. On failure the CLI prints counts by error kind with one example group each; full errors are in the `processing_failures` table for the reported run. Results are written to SQLite on one thread in group order, so a group's result commits as soon as it and every earlier group are done. Unchanged, successful groups are skipped unless `--reprocess` is supplied. Prompt, model, schema, and source fingerprints determine whether a group is unchanged. A reply edit invalidates its top-level group. If one group fails, successful groups still commit, the failed group's previous result remains in SQLite, the processing run is marked incomplete, and the CLI exits nonzero. Oversized items or ancestor chains produce a recorded failure rather than truncated input.

Rig extraction uses a process-wide Tokio runtime that remains driven for the lifetime of its shared HTTP connection pool. A processing run coordinates retry cooldowns across its workers so that new attempts wait for the shared backoff deadline after transient failures; already running requests may finish. Rig internal retries are disabled so provider attempts pass through the shared cooldown. Transport diagnostics retain bounded error causes with the configured credential and endpoint redacted.

Each model invocation receives one top-level comment subtree, or bounded chunks with root and ancestor context. All source IDs must be accounted for. HN HTML is converted to plain text with full link destinations before extraction and validation. Structured JSON output contains project mentions with a verbatim `source_excerpt`, a project-centric description, optional problem/audience text, category and audience slugs, and additional explicitly named `builders`. The source author is assumed to be a builder; `builders` excludes that author. Missing names may become short descriptive names. Replies provide context or introduce their own projects with their actual source IDs; requests and separate supporting contributions are no longer extracted.

Local validation failures allow up to two correction attempts per batch (`Options::max_validation_corrections`, default 2, capped at 2). Correction prompts retain the original source records and carry validation feedback and a bounded rejected response as untrusted JSON data. Every corrected response undergoes the same validation before any group results are saved. Exhausted validation errors record the source ID and bounded rejected field in the private `processing_failures.error` column; URL diagnostics omit credentials, queries, and fragments. Validation failures remain separate from provider failures even when rejected text contains transport-error wording. Retry-policy changes do not invalidate successful groups or require renewed approval by themselves.

Categories and audiences are proposed classifications. Audience slugs come from [audiences.md](audiences.md), with zero to five distinct values only when the source mentions the audience. The prompt and schema versions change so existing groups are processed again on the next explicit process run. Existing approved records are not silently reclassified or approved by a database migration. New extraction through the manual CLI still requires review; `wrkgn-monthly` automatically approves after each successful thread. URLs and excerpts are validated against the supplied source; safe project-origin normalization is permitted for multiple same-origin demo URLs. Shared-hosting project paths must be preserved.

New mentions are pending review. `review pending` prints one readable card per mention: extracted fields with corrections applied, labels, builders, links, and source evidence. Show HN cards display every stored evidence quotation with its source field, actual author, original date, HN source link, and supported fields. Authorized-builder evidence links to the submitter authorization source; its identifying quotation is also displayed among the evidence. Monthly cards retain the single-comment excerpt and context without invented field-level support. It ends with the pending IDs as compact ranges. Set `COLUMNS` to change the card width; `--json` retains the existing one-object-per-line fields, applies corrections, and adds an `evidence` array (empty for legacy monthly mentions) with source metadata and optional authorization source IDs. Review output does not include private fingerprints, raw payloads, or processing diagnostics. `approve` takes IDs and inclusive ranges (`12-30 34`) and approves them in one transaction. Non-pending IDs inside a range are skipped. A range with no pending mentions fails without approving anything. Review the cards before approving a range; corrections persist by mention key across reruns. Successful reprocessing returns changed material to pending review. Source and project suppression survive reruns. There is no command to undo suppression yet; inspect and edit local SQLite deliberately if a suppression was erroneous.

## Automated monthly binary

`cargo run --bin wrkgn-monthly --` discovers and handles monthly discussions one thread at a time, reusing `Collector`, `process_thread_with`, thread-scoped approval, and the standard public exporter. It defaults to the current UTC month through 23 months earlier. Use `--from YYYY-MM --through YYYY-MM` for an inclusive range, plus the existing `--db`, `--export`, and `--concurrency` settings. `default-run = "wrkgn"` keeps existing `cargo run -- ...` commands unchanged.

Discovery queries the HN Algolia story-title index by year, pages through all hits, rejects explicitly non-exhaustive results, filters whole titles locally, deduplicates IDs, and accepts full month names or three-letter abbreviations (also `Sept`), optional `Ask HN:`, and question marks. Network errors and 429/5xx responses receive up to three attempts. Indexing delays can hide new discussions until a later run. Empty months are skipped; all matching threads are processed before advancing to the previous month.

The collector refreshes the full tree so new replies, edits, and deletions retain existing semantics. The processor skips unchanged successful groups using its existing source/prompt/schema/model checks. After a complete successful thread pass, its pending mentions are automatically approved, including earlier pending results; other threads are unaffected. Corrections, matching, and suppression persist. Export remains cumulative across all approved threads. Failures stop traversal with a nonzero exit; reruns resume saved successful groups. A partial processing pass attempts a freshness-filtered export but does not auto-approve the thread. Approval and export commit separately, so rerun after an export failure. The workflow never stages website assets. See [the maintainer runbook](../MANAGE.md#automated-monthly-workflow).

Offline tests inject paginated discovery responses, HN fixtures, and a deterministic model; ordinary checks never call live HN or LLM services.

## Public snapshot contract

`export` reads all approved projects from one SQLite transaction, applies corrections and suppression, and creates `snapshots/<version>/manifest.json`, `discovery.json`, and `projects/<slug>.json` under the export directory. `current.json` is replaced atomically and points to the completed snapshot. The version hashes both discovery and detail data. Astro should read `current.json` once per build, then read all JSON through its selected manifest. It must never copy the working database or raw payloads.

Discovery records have stable `id` and `slug`, card fields (`name`, `description`, `problem`, `audience`), `builder_usernames`, `categories`, `audience_labels`, `latest_shared_at`, `comment_count`, and `searchable_text`. They are ordered by latest original mention date, then project ID. Public schema 5 retains `comment_count` from version 4: distinct replies beneath a project’s published source comments, including nested replies, excluding deleted, dead, absent, and source-suppressed replies. Overlapping subtrees are deduplicated. Counts use stored source at export time; no HN or model request is needed. Comment vote counts are unavailable from HN. Detail records have `id`, `slug`, `name`, and dated `mentions`; each mention includes its source ID and URL, author, links, verbatim source excerpt, and builder usernames. Dates are Unix seconds from HN source records. A current source edit, deletion, missing item, or suppression hides affected content; a failed rerun does not republish stale group results. Static HTML and modal data should both come from this snapshot.

The maintainer has inspected many detail pages and the project list and considers extraction quality sufficient to offer the explicit automated monthly workflow. Schema validity alone does not establish extraction accuracy; manually inspect descriptions, labels, invitations, provenance, and grouping before publication.

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

`npm run stage` takes one export directory containing `current.json`. It validates public schema version 5 (and accepts existing version 2, 3, and 4 snapshots for compatibility), the selected manifest (including the category and audience taxonomies for versions 3 through 5), every discovery record, every referenced project detail, and the public field allowlist. It copies only those files into a replacement staged snapshot; previous staged versions are removed. A missing or invalid export fails before replacing the staged data. `npm run dev` and `npm run build` consume the staged snapshot without running Rust, contacting HN or a model, or reading local credentials or SQLite. Static output goes to `website/dist/`; the versioned browser data is under `/data/<version>/`.

Cloudflare repository builds should use `website/` as the build root, `npm run build` as the command, and `dist/` as the output directory. Deployment configuration and publication remain separate from this local setup. A real correction and removal contact destination must be supplied before launch; do not publish a placeholder route.

The directory renders ordinary list and project links without JavaScript. After discovery data loads, search and Category and Audience appear in the shared fixed left sidebar on wide screens. The first 30-project page is `/`, with `/projects/` as an alias; later pages use `/projects/page/N/`. Direct detail pages offer an ordinary filter form that returns to `/`. On narrow screens (below 768px), a Filters button in the primary navigation opens the same controls in an opaque, fixed full-screen overlay. Its contents scroll independently above a pinned Clear all / Close footer, while the underlying page is locked. Close or Escape restores page scroll and focus. Resizing to desktop closes the overlay and restores the sidebar; Close is hidden on desktop. On listing pages, the button and controls stay hidden when discovery data cannot load; detail-page controls remain available as a normal form. Search matches the exported `searchable_text` for every published project. Selected filter values are OR within one dimension and AND across dimensions and the text query. The Sort by dropdown on the right of the count row offers comment count in both directions and date in both directions, defaulting to recent first. Sorting applies to all matches before pagination, with newest date then project ID resolving equal counts and project ID resolving equal dates. A sort change resets page one; Clear all also resets the default order. Legacy snapshots without counts offer date orders only. Criteria, sort, and interactive page state use URL parameters; invalid values are dropped and out-of-range pages clamp to the last available page. Opening a card fetches versioned detail JSON and the matching standalone page HTML, then displays its shared detail markup in a modal. A failed detail fetch navigates to the ordinary project page.

### Manual browser checklist

The maintainer will perform browser interaction checks. For a local synthetic dataset, run `npm run stage -- tests/fixtures/representative` from `website/`, start `npm run dev`, and then restore the approved or empty snapshot after testing. Check:

- `/` and `/projects/` show the same 13 cards, while `/projects/page/2/` is absent for this fixture; the approved 83-project snapshot generates pages of 30, 30, and 23 cards, with ordinary links; opening a direct `/projects/<slug>/` URL works with JavaScript disabled.
- Search from page 1 finds both records containing `samplebuilder`; Category and Audience selections combine as documented. Try all four sort orders, including with active filters and on a later page. Reload and browser back/forward restore query, selections, sort, and page; Clear all resets them. Try an empty result and a malformed or out-of-range `page` parameter.
- Open a card in the modal, inspect dated mentions and builder attribution, then close with the button, Escape, and browser Back. Confirm Forward reopens it and focus returns to the card after closing. Modified click should open the standalone page. Simulate a failed detail JSON request and confirm navigation falls back to that page.
- At wide viewport sizes, check the left sidebar, vertical search and filters, compact heading, and card widths. At narrow sizes, check that the navigation Filters button opens an opaque full-screen overlay, updates its expanded state, and keeps keyboard focus inside. Scroll through both filter groups and check that only the contents move while Clear all and Close stay pinned. Clear all should reset selections without closing; Close and Escape should restore page scroll and focus to Filters. Resize while the panel is open and closed, and verify controls and selected values remain usable. With JavaScript disabled or the discovery request failed, check that ordinary cards and pagination work while the filter controls stay hidden. Check modal fit, HTTP(S) source links, and dated source attribution.

Before launch, replace the synthetic/empty staged data with a reviewed, approved export, manually assess extraction quality on the three-thread pilot, and set a real correction/removal contact route.

## Project extraction schema migration

Database migration 6 stores private author and normalized URL aliases for project identity. It seeds aliases from existing mention links on first open and records them before replacement, so a successful zero-mention rerun does not erase a reviewed match or suppression. It does not modify existing project IDs, slugs, review decisions, or staged exports. Existing duplicates require source inspection and explicit `review match`; opening the database does not reconcile them automatically.

Database migration 5 adds source excerpts and additional builder records without deleting legacy request/supporting history. Public schema 3 omits requests, supporting contributions, and Looking for, and adds audience taxonomy metadata, source excerpts, and per-mention builder usernames. Missing legacy source excerpts or unknown legacy audience labels cause export to stop with instructions to reprocess; labels and approvals are not silently rewritten. Reprocess and review before staging a new export. Existing staged schema-2 data remains readable by the website.


## Show HN storage and public evidence

Database migration 7 adds a persisted extraction mode per processed thread (`monthly` or `show_hn`). Existing processing runs are backfilled as monthly without changing their fingerprints, prompt/schema versions, approvals, identities, or slugs. Collected threads with no processing history remain unassigned. Mode assignment rejects conflicting interpretations; collection alone never chooses a mode.

`mention_evidence` stores each supporting source ID, source field (`title`, `url`, or `body`), verbatim value, supported fields, source fingerprint, and optional submitter-authorization dependency and fingerprint. Existing monthly records retain their single-comment excerpt without invented field-level support. Show HN chunk results are cached privately in `show_chunk_cache`. Each chunk key includes its source and ancestor state, repeated submitter context, suppression, model, and processing versions; consolidation inherits its inputs' dependencies. Every consolidation level receives the actual authors, original timestamps, and parent IDs for its evidence and authorization sources, plus the story submitter identity; it does not receive whole source bodies. The complete prompt including metadata must fit the input bound. Explicit submitter corrections take precedence only when their chronology and meaning are clear; ambiguous disputes are omitted. Independent reply edits or appended branches reuse unaffected chunks, while the final result retains a whole-thread freshness identity. `show_processing_results` retains a successful no-project reason. These tables are local processing state and never website assets.

New public snapshots use schema 5. Each mention retains its anchor author, original sharing date, source URL, and legacy excerpt field, and adds an `evidence` array. Each evidence entry contains `source_item_id`, `source_field`, `value`, `supports`, `author`, `shared_at`, and `source_url`. Evidence authors and dates come from the actual source item; supporting replies do not change the story's sharing date. Legacy monthly evidence uses `body` and an empty `supports` array. The public array exposes concise excerpts rather than full discussions. Source and authorization edits, confirmed removals, deletion, or suppression conservatively withhold the affected mention until valid processing replaces it. Private authorization fingerprints and processing diagnostics remain excluded.

The website explicitly supports public versions 2, 3, 4, and 5, rejecting unsupported versions. Staging, standalone pages, and modal readers use the same validated contract; schema-5 details render each evidence field with its actual author, date, and HN source link. Title evidence is presented as a title rather than a manufactured comment excerpt. Builds and staging make no live HN or model calls.
