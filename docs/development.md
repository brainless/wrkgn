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

`Collector` accepts a supplied thread item ID and an `HnApi` transport. The production transport reads the public HN item endpoint with a 15-second request timeout; tests inject offline fixture responses. The collector fetches the root and each listed descendant, preserving the `kids` order as sibling positions and validating child parent IDs. At most eight item fetches run concurrently by default. Transient network errors, HTTP 429, and HTTP 5xx responses receive up to three attempts with bounded delay.

A JSON `null`, malformed item, failed request, duplicate child reference, or parent mismatch makes the run incomplete and identifies the affected item in `fetch_failures`. An explicit `deleted` or `dead` field is stored as source state. A failed or null fetch never changes that state. No live collection runs during builds or tests; a deliberate live smoke test awaits a supplied monthly discussion ID.

## Processing and review

Set `WRKGN_MODEL` to an OpenAI completion model name and `OPENAI_API_KEY` in the local environment. Keep credentials out of Git and logs. Rig is pinned to upstream commit `7dfd8a4229a9d89af159489687cb14b404b2871d`. Model calls occur only during an explicit `process` command. Tests use a deterministic `Model` fixture and never need credentials.

```sh
cargo run -- collect --thread <hn-item-id>
cargo run -- process --thread <hn-item-id>
cargo run -- process --thread <hn-item-id> --reprocess
cargo run -- review pending
cargo run -- review approve <mention-id>
cargo run -- review correct <mention-key> description "Reviewed summary"
cargo run -- review suppress-source <hn-item-id> "removal request"
cargo run -- review suppress-project <project-id>
cargo run -- review match <from-project-id> <to-project-id>
cargo run -- export
```

`--db <path>` selects the local SQLite database. `--export <directory>` selects the public export directory; `WRKGN_EXPORT_PATH` overrides the default `local-data/public-export`. Collection must complete before processing. Unchanged, successful groups are skipped unless `--reprocess` is supplied. Prompt, model, schema, and source fingerprints determine whether a group is unchanged. A reply edit invalidates its top-level group. If one group fails, successful groups still commit, the failed group's previous result remains in SQLite, the processing run is marked incomplete, and the CLI exits nonzero. Oversized items or ancestor chains produce a recorded failure rather than truncated input.

Each model invocation receives one top-level comment subtree, or bounded chunks with root and ancestor context. All source IDs must be accounted for. Typed results are checked against the supplied source IDs, URL presence, builder attribution, and exact evidence for explicit requests. Categories and audience labels are classifications; only evidenced requests become Looking for values. A repeat mention with an exact URL and the same supported builder can reuse a project. Other cross-conversation matches remain separate until `review match` explicitly merges their project identities. Project slugs are stored in SQLite.

New mentions are pending review. `review pending` shows their IDs, keys, descriptions, source IDs, and sharing dates. `approve` publishes one pending mention; corrections persist by mention key across reruns. Successful reprocessing returns changed material to pending review. Source and project suppression survive reruns. There is no command to undo suppression yet; inspect and edit local SQLite deliberately if a suppression was erroneous.

## Public snapshot contract

`export` reads all approved projects from one SQLite transaction, applies corrections and suppression, and creates `snapshots/<version>/manifest.json`, `discovery.json`, and `projects/<slug>.json` under the export directory. `current.json` is replaced atomically and points to the completed snapshot. The version hashes both discovery and detail data. Astro should read `current.json` once per build, then read all JSON through its selected manifest. It must never copy the working database or raw payloads.

Discovery records have stable `id` and `slug`, card fields (`name`, `description`, `problem`, `audience`), `builder_usernames`, `categories`, `audience_labels`, `looking_for`, `latest_shared_at`, and `searchable_text`. They are ordered by latest original mention date, then project ID. Detail records have `id`, `slug`, `name`, and dated `mentions`; each mention includes its source ID and URL, author, links, evidenced requests, and attributed supporting contributions. Dates are Unix seconds from HN source records. A current source edit, deletion, missing item, or suppression hides affected content; a failed rerun does not republish stale group results. Static HTML and modal data should both come from this snapshot.

A live model quality check and the three-thread pilot remain deferred until real thread IDs, credentials, and deliberate execution are available. Schema validity alone does not establish extraction accuracy; manually inspect descriptions, labels, invitations, provenance, and grouping before publication.

## Static website (Epic 03)

The Astro site lives in `website/`. It reads only `website/data/public/`, which is a selected, tracked public snapshot. An empty snapshot is committed initially; a synthetic representative fixture is in `website/tests/fixtures/representative/` for offline UI work and must not be presented as real HN content.

After reviewing and approving mentions locally, run `cargo run -- export` from the repository root. Then explicitly stage the completed export and build from `website/`:

```sh
cd website
npm ci
npm run stage -- ../local-data/public-export
npm run dev
npm run build
npm test
```

`npm run stage` takes one export directory containing `current.json`. It validates schema version 1, the selected manifest, every discovery record, every referenced project detail, and the public field allowlist. It copies only those files into a replacement staged snapshot; previous staged versions are removed. A missing or invalid export fails before replacing the staged data. `npm run dev` and `npm run build` consume the staged snapshot without running Rust, contacting HN or a model, or reading local credentials or SQLite. Static output goes to `website/dist/`; the versioned browser data is under `/data/<version>/`.

Cloudflare repository builds should use `website/` as the build root, `npm run build` as the command, and `dist/` as the output directory. Deployment configuration and publication remain separate from this local setup. A real correction and removal contact destination must be supplied before launch; do not publish a placeholder route.

The directory renders ordinary list and project links without JavaScript. With JavaScript, search matches the exported `searchable_text` for every published project. Category, Audience, and Looking for are the three filter dimensions: selected values are OR within one dimension and AND across dimensions and the text query. Criteria and interactive page state use URL parameters; invalid values are dropped and out-of-range pages clamp to the last available page. Opening a card fetches versioned detail JSON and the matching standalone page HTML, then displays its shared detail markup in a modal. A failed detail fetch navigates to the ordinary project page.

### Manual browser checklist

The maintainer will perform browser interaction checks. For a local synthetic dataset, run `npm run stage -- tests/fixtures/representative` from `website/`, start `npm run dev`, and then restore the approved or empty snapshot after testing. Check:

- `/projects/` and `/projects/page/2/` show 12 and 1 cards respectively, with ordinary links; opening a direct `/projects/<slug>/` URL works with JavaScript disabled.
- Search from page 1 finds both records containing `samplebuilder`; Category, Audience, and Looking for selections combine as documented. Reload and browser back/forward restore query, selections, and page; Clear all resets them. Try an empty result and a malformed or out-of-range `page` parameter.
- Open a card in the modal, inspect dated mentions and supporting attribution, then close with the button, Escape, and browser Back. Confirm Forward reopens it and focus returns to the card after closing. Modified click should open the standalone page. Simulate a failed detail JSON request and confirm navigation falls back to that page.
- At narrow and wide viewport sizes, check readable content, usable filters, keyboard focus, and modal fit. Confirm source links use HTTP(S) and historical invitations show their original dates.

Before launch, replace the synthetic/empty staged data with a reviewed, approved export, manually assess extraction quality on the three-thread pilot, and set a real correction/removal contact route.
