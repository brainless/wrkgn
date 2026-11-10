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

`WRKGN_PROVIDER` defaults to `openai`. For OpenAI, set `OPENAI_API_KEY` and `WRKGN_MODEL` to an OpenAI completion model name. For Neuralwatt, set `WRKGN_PROVIDER=neuralwatt` and `NEURALWATT_API_KEY`; `WRKGN_MODEL` defaults to `glm-5.3-flash`, and `NEURALWATT_BASE_URL` defaults to `https://api.neuralwatt.com/v1`. Neuralwatt uses Rig's OpenAI-compatible chat route. Provider, model, and endpoint identify processed groups, so switching any of them triggers extraction again. Rig is pinned to upstream commit `7dfd8a4229a9d89af159489687cb14b404b2871d`. Model calls occur only during an explicit `process` command. Tests use a deterministic `Model` fixture and never need credentials.

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

`--db <path>` selects the local SQLite database. `--export <directory>` selects the public export directory; `WRKGN_EXPORT_PATH` overrides the default `local-data/public-export`. `process` never fetches from HN: it reads stored source and requires the latest collection run for the thread to be `complete`, otherwise it fails and asks you to rerun `collect`. Run `collect` first, then `process`. `--concurrency <n>` (default 10) sets how many conversation groups are extracted in parallel; lower it if the provider rate-limits. Rate-limit (429), provider 5xx, and connection errors are retried up to six attempts with jittered exponential backoff from 1s to 30s. After three groups exhaust their retries on such errors, no new groups are started; the rest are reported as not attempted, and rerunning without `--reprocess` resumes, skipping groups completed with the current prompt, schema, and model. Progress (`Extracting n/total groups (pct%), k failed`) goes to stderr, in place on a terminal and at 10% steps otherwise. On failure the CLI prints counts by error kind with one example group each; full errors are in the `processing_failures` table for the reported run. Results are written to SQLite on one thread in group order, so a group's result commits as soon as it and every earlier group are done. Unchanged, successful groups are skipped unless `--reprocess` is supplied. Prompt, model, schema, and source fingerprints determine whether a group is unchanged. A reply edit invalidates its top-level group. If one group fails, successful groups still commit, the failed group's previous result remains in SQLite, the processing run is marked incomplete, and the CLI exits nonzero. Oversized items or ancestor chains produce a recorded failure rather than truncated input.

Each model invocation receives one top-level comment subtree, or bounded chunks with root and ancestor context. All source IDs must be accounted for. HN stores comment bodies HTML-escaped, so the model receives plain text instead (`html::plain_text`): entities decoded, `<p>` as a blank line, links replaced by their full target URL (HN truncates long link text with `...`), and other tags dropped. The input size limit measures this text. Stored source data, fingerprints, and raw payloads are unchanged. Typed results are validated in Rust against the same plain text and the supplied source IDs. A URL must match a URL or bare domain in the source (case-insensitive host, trailing slash ignored, `https://` accepted on a bare domain), and request evidence and supporting excerpts must be quoted from it, ignoring whitespace and curly-versus-straight quotes. Request kinds are limited to feedback, contributors, and collaborators by the output schema. The model reports only whether the item's author states they built the project (`author_is_builder`); the stored builder is then that item's HN author, and a commenter is never assumed to be the builder. Omitted arrays in model output are read as empty. Categories and audience labels are classifications; only evidenced requests become Looking for values. A repeat mention with an exact URL and the same supported builder can reuse a project. Other cross-conversation matches remain separate until `review match` explicitly merges their project identities. Project slugs are stored in SQLite.

New mentions are pending review. `review pending` prints one readable card per mention: extracted fields with corrections applied, labels, Looking for, links, supporting replies, and an excerpt of the source comment. It ends with the pending IDs as compact ranges. Set `COLUMNS` to change the card width; `--json` prints the previous one-object-per-line output. `approve` takes IDs and inclusive ranges (`12-30 34`) and approves them in one transaction. Non-pending IDs inside a range are skipped. A range with no pending mentions fails without approving anything. Review the cards before approving a range; corrections persist by mention key across reruns. Successful reprocessing returns changed material to pending review. Source and project suppression survive reruns. There is no command to undo suppression yet; inspect and edit local SQLite deliberately if a suppression was erroneous.

## Public snapshot contract

`export` reads all approved projects from one SQLite transaction, applies corrections and suppression, and creates `snapshots/<version>/manifest.json`, `discovery.json`, and `projects/<slug>.json` under the export directory. `current.json` is replaced atomically and points to the completed snapshot. The version hashes both discovery and detail data. Astro should read `current.json` once per build, then read all JSON through its selected manifest. It must never copy the working database or raw payloads.

Discovery records have stable `id` and `slug`, card fields (`name`, `description`, `problem`, `audience`), `builder_usernames`, `categories`, `audience_labels`, `looking_for`, `latest_shared_at`, and `searchable_text`. They are ordered by latest original mention date, then project ID. Detail records have `id`, `slug`, `name`, and dated `mentions`; each mention includes its source ID and URL, author, links, evidenced requests, and attributed supporting contributions. Dates are Unix seconds from HN source records. A current source edit, deletion, missing item, or suppression hides affected content; a failed rerun does not republish stale group results. Static HTML and modal data should both come from this snapshot.

Live extraction has started on the first pilot thread; manual quality assessment across the three-thread pilot is still outstanding. Schema validity alone does not establish extraction accuracy; manually inspect descriptions, labels, invitations, provenance, and grouping before publication.

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

`npm run stage` takes one export directory containing `current.json`. It validates schema version 2, the selected manifest (including the category taxonomy), every discovery record, every referenced project detail, and the public field allowlist. It copies only those files into a replacement staged snapshot; previous staged versions are removed. A missing or invalid export fails before replacing the staged data. `npm run dev` and `npm run build` consume the staged snapshot without running Rust, contacting HN or a model, or reading local credentials or SQLite. Static output goes to `website/dist/`; the versioned browser data is under `/data/<version>/`.

Cloudflare repository builds should use `website/` as the build root, `npm run build` as the command, and `dist/` as the output directory. Deployment configuration and publication remain separate from this local setup. A real correction and removal contact destination must be supplied before launch; do not publish a placeholder route.

The directory renders ordinary list and project links without JavaScript. After discovery data loads, search and Category, Audience, and Looking for appear in a left sidebar on wide screens. On narrow screens, a Filters button in the primary navigation opens the same controls above the results. The button and controls stay hidden when discovery data cannot load. Search matches the exported `searchable_text` for every published project. Selected filter values are OR within one dimension and AND across dimensions and the text query. Criteria and interactive page state use URL parameters; invalid values are dropped and out-of-range pages clamp to the last available page. Opening a card fetches versioned detail JSON and the matching standalone page HTML, then displays its shared detail markup in a modal. A failed detail fetch navigates to the ordinary project page.

### Manual browser checklist

The maintainer will perform browser interaction checks. For a local synthetic dataset, run `npm run stage -- tests/fixtures/representative` from `website/`, start `npm run dev`, and then restore the approved or empty snapshot after testing. Check:

- `/projects/` and `/projects/page/2/` show 12 and 1 cards respectively, with ordinary links; opening a direct `/projects/<slug>/` URL works with JavaScript disabled.
- Search from page 1 finds both records containing `samplebuilder`; Category, Audience, and Looking for selections combine as documented. Reload and browser back/forward restore query, selections, and page; Clear all resets them. Try an empty result and a malformed or out-of-range `page` parameter.
- Open a card in the modal, inspect dated mentions and supporting attribution, then close with the button, Escape, and browser Back. Confirm Forward reopens it and focus returns to the card after closing. Modified click should open the standalone page. Simulate a failed detail JSON request and confirm navigation falls back to that page.
- At wide viewport sizes, check the left sidebar, vertical search and filters, compact heading, and card widths. At narrow sizes, check that the navigation Filters button opens and closes the same controls above results, updates its expanded state, and supports keyboard focus. Resize while the panel is open and closed, and verify controls and selected values remain usable. With JavaScript disabled or the discovery request failed, check that ordinary cards and pagination work while the filter controls stay hidden. Check modal fit, HTTP(S) source links, and dated historical invitations.

Before launch, replace the synthetic/empty staged data with a reviewed, approved export, manually assess extraction quality on the three-thread pilot, and set a real correction/removal contact route.
