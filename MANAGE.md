# Managing wrkgn.com

The site is hosted on Cloudflare at **https://wrkgn.com**, with the repository integration already working. Collection, model processing, review, and SQLite writes run on the maintainer's computer. Publication follows this flow:

```text
collect → process → review / approve → export → stage → inspect / build → commit / push → Cloudflare
```

## Automated monthly workflow

The separate `wrkgn-monthly` binary reuses the existing collector, incremental processor, project matching, approval state, and exporter. The original `wrkgn` binary retains its manual review workflow.

```sh
cargo run --bin wrkgn-monthly --
# Choose an inclusive range, newest month first:
cargo run --bin wrkgn-monthly -- --from 2026-11 --through 2026-09
# Refresh just one month:
cargo run --bin wrkgn-monthly -- --from 2026-11 --through 2026-11 --concurrency 3
```

By default it starts at the current UTC month and covers 24 months inclusive. It searches the [HN Algolia index](https://hn.algolia.com/api) for monthly “What are you working on” story titles, accepting full and abbreviated month names (`November`, `Nov`, and `September`/`Sep`/`Sept`), optional `Ask HN:` prefixes, and question marks. It checks every search page and excludes unrelated titles. Every matching thread in a month is handled individually, in descending item-ID order, before proceeding to the previous month. Months without matches are skipped. Discovery depends on Algolia indexing; rerun later for newly indexed threads.

Each thread refreshes the full reply tree to save new comments and detect edits, deletions, and new replies. Existing fingerprint/version checks skip unchanged successful conversation groups. After the whole thread processes successfully, its pending mentions are automatically approved, including previously pending results for that thread. Pending mentions in other threads remain pending. Corrections, matching, and suppression retain their existing behavior. The exporter writes all approved public records across the database after each thread, retaining earlier threads in the snapshot.

A discovery, collection, processing, or export failure stops traversal and exits nonzero. Successful groups stay saved; rerun the same command to resume. Incomplete processing does not auto-approve that thread, but attempts export using existing approvals and freshness checks to remove stale public results. Approval and export are separate commits: if export fails after approval, rerunning retries export. The command uses the same `.env`, `--db`, `--export`, and `--concurrency` settings as `wrkgn`. It never stages or deploys the website. After it succeeds, run `npm run stage -- ../local-data/public-export` in `website/`, then inspect/build and publish as below.

## Show HN workflow

The separate `wrkgn-show-hn` command refreshes a story and its entire discussion, extracts its primary project, and exports approved data. Choose one story or an inclusive UTC date range (earlier `--from`, later `--through`):

```sh
cargo run --bin wrkgn-show-hn -- --thread <hn-item-id>
cargo run --bin wrkgn-show-hn -- --from 2026-11-01 --through 2026-11-07
# Deliberately authorize story-scoped approval after successful processing:
cargo run --bin wrkgn-show-hn -- --from 2026-11-01 --through 2026-11-07 --auto-approve
```

Results await manual review by default. Use the existing `wrkgn review pending`, correction, approval, suppression and matching commands. `--auto-approve` approves only pending mentions of each successfully processed story. `--db`, `--export`, `--concurrency`, provider configuration and `.env` work as with the existing CLIs; `--reprocess` deliberately reruns extraction. Rerun without it to resume and reuse unchanged successful work. Stories finish newest first, one at a time. A failure stops the command; completed work remains saved.

Date discovery uses Algolia, checks all pages, and partitions crowded UTC timestamp windows. Index lag means a later rerun may discover additional stories. Collected HN content supplies extraction data; linked websites are not crawled. Only live stories with a `Show HN:` prefix, author and submission date qualify. The persisted extraction mode prevents monthly and Show HN workflows from interpreting the same thread differently. Ordinary `wrkgn process` dispatches from that mode and still never fetches HN.

Show HN facts use the title, URL and submitter explanations, plus additional builders explicitly identified by the submitter. Other participants provide context. The story retains its original date and submitter attribution; each evidence entry keeps its own source author/date. Source edits, deletion, suppression and lost builder authorization withhold stale results even if a rerun fails. Oversized discussions use bounded resumable chunks; incomplete work cannot publish a partial replacement. The initial strategy repeats submitter context and required ancestors to preserve builder authorization. If that context alone exceeds the 30 KB input bound, or consolidation cannot fit its bound, processing fails explicitly without truncating sources. A successful no-project result retires previous mentions while retaining identity history.

Start with a small live pilot: title/link-only story, text story, submitter clarifications, and an oversized discussion. Inspect factual support and attribution before broad date discovery or automatic approval. These quality checks and browser interactions remain maintainer tests. Staging and deployment remain explicit steps below.

## Local setup and paths

Run the Rust commands below from the repository root. `cargo run --bin wrkgn --` runs the current CLI source; if you have installed an up-to-date `wrkgn` binary, you can use `wrkgn` instead.

- Rust uses the pinned nightly toolchain and Cranelift configuration. See [development setup](docs/development.md#rust-toolchain).
- Copy `.env.example` to `.env` if needed and configure one provider's credentials locally. The CLI loads `.env` from the current directory; shell variables take precedence. Never commit credentials.
- `WRKGN_PROVIDER` defaults to `openai`, requiring `OPENAI_API_KEY` and `WRKGN_MODEL`. For Neuralwatt, set `WRKGN_PROVIDER=neuralwatt` and `NEURALWATT_API_KEY`; its default model is `glm-5.3-flash`.
- The default database is `local-data/wrkgn.sqlite`. Override it with `WRKGN_DB_PATH` or `--db <path>`.
- The default export directory is `local-data/public-export`. Override it with `WRKGN_EXPORT_PATH` or `--export <directory>` on commands that export. If using a custom path, stage that directory below.
- The website requires Node >=22.12. Run `npm ci` from `website/` on initial setup or after dependency changes.

Keep the local database: it contains source history, review decisions, corrections, suppression, and stable project identities. Back it up before deliberate manual database edits, with pipeline commands stopped. Generated exports and staged website snapshots can be replaced without resetting SQLite.

## 1. Collect a thread or refresh its source

Use the item ID from the HN discussion URL, for example `https://news.ycombinator.com/item?id=50417407`:

```sh
cargo run --bin wrkgn -- collect --thread 50417407
```

This fetches the post and its reply tree and stores them locally. Repeat the same command to refresh edits, deletions, and replies. In the manual `wrkgn` CLI, only `collect` fetches HN data. It does not extract projects or update the staged website.

Processing requires the latest collection run for that thread to be complete. If collection fails or is incomplete, rerun `collect` before processing. Repeat collection and processing separately for each thread, or use `wrkgn-monthly` for the automated path.

## 2. Process stored comments

```sh
cargo run --bin wrkgn -- process --thread 50417407
```

This calls the configured model using stored source, without fetching HN. One extraction agent handles a top-level comment and its reply subtree, with bounded chunks for oversized groups. Rust validates the results. New mentions are pending review; extraction success is not publication approval.

The extractor returns project descriptions, verbatim project excerpts, URLs, categories, audience labels, and explicitly named additional builders. The application assumes the source author is a builder. Missing audiences stay unknown. Requests and separate supporting contributions are outside the current extraction scope. See [categories](docs/categories.md) and [audiences](docs/audiences.md).

Successful unchanged groups are skipped. Changes to source, prompt/schema version, or provider/model identity cause affected groups to run again automatically. After an interruption or partial failure, repeat the ordinary command to resume. Previous successful results are retained on extraction failure; publication checks prevent stale source results from being republished.

To reduce parallel model calls (default: 10):

```sh
cargo run --bin wrkgn -- process --thread 50417407 --concurrency 3
```

Rate-limit and transport failures receive retries with backoff shared across workers, pausing new requests while the provider recovers. Persistent failures stop new work; inspect the reported failures and resume once the provider issue is resolved.

If a structured response fails local provenance validation, the extractor gets up to two correction attempts with the original source and validation feedback. Every correction must pass the same checks. Exhausted failures retain previous successful results. Private failure rows include the source ID and a bounded rejected value; transport errors include bounded, credential-redacted causes. Inspect them using the SQL query printed with the run ID. These diagnostics are excluded from public exports.

### Force reprocessing

```sh
cargo run --bin wrkgn -- process --thread 50417407 --reprocess
```

Use this to deliberately rerun all groups, including current successful ones, such as when evaluating extraction quality. A properly versioned prompt/schema update already invalidates older results, so an ordinary `process` run is enough to migrate them. Resume a failed forced run **without** `--reprocess` to skip groups that have now succeeded.

Successful replacement results return to pending review. Corrections and suppression decisions survive reruns; inspect the new output before approving it again.

## 3. Review and approve

```sh
cargo run --bin wrkgn -- review pending
cargo run --bin wrkgn -- review pending --json
```

The readable cards show extracted fields, labels, builders, and source context. Show HN cards include each stored evidence quotation with its source field, actual author/date, HN link, supported fields, and authorization-source link for additional builders; the submitter identifying statement appears among those quotations. Monthly cards retain their original single-comment excerpt and context. The JSON form retains mention keys for corrections and adds the same evidence records with authorization source IDs; corrections apply in both views. Pending review covers the database, not just the last processed thread. Check source attribution, excerpt accuracy, descriptions, labels, URLs, and possible duplicate projects. Manually assess representative cases across the three-thread pilot before scaling up collection.

Approve only reviewed mention IDs, individually or as inclusive ranges. These IDs are examples; use the IDs shown by your own review:

```sh
cargo run --bin wrkgn -- review approve 12-30 34
```

Non-pending IDs inside a range are skipped. A range containing no pending mentions fails the approval transaction.

### Correct extracted text

Corrections take a **mention key**, rather than the numeric approval ID:

```sh
cargo run --bin wrkgn -- review correct <mention-key> description "Reviewed factual description"
```

Supported fields are `name`, `description`, `problem`, `audience`, and `builder_username`. Audience text is separate from taxonomy labels. There is no general CLI editor for categories, audience labels, excerpts, or additional builders; fix extraction and reprocess when those need changing. A correction does not approve a pending mention.

### Remove content or match duplicate projects

```sh
cargo run --bin wrkgn -- review suppress-source <hn-item-id> "removal request"
cargo run --bin wrkgn -- review suppress-project <project-id>
cargo run --bin wrkgn -- review match <from-project-id> <to-project-id>
```

Source suppression excludes that source's content from publication; project suppression excludes the project. Suppression survives reprocessing. There is currently no CLI command to undo it; an erroneous suppression requires deliberate local SQLite inspection and editing.

Match only when evidence shows two project records represent the same project. Matching moves mentions from the first project to the second and records the identity mapping for reruns. It is separate from extraction and does not approve mentions.
Both projects must have the same known HN source author; suppressed projects cannot be manually matched. Automatic matching also requires that author and a shared explicit URL. URL order and an ordinary trailing slash do not affect matching, while URL path case, query, fragment, and distinct paths remain significant. Ambiguous URL matches remain separate for review; processing stops for the affected group if an ambiguous candidate is suppressed. Previously split projects are not silently merged by opening the database or exporting: inspect duplicate candidates and use `review match` only after checking their sources. Keep the database backed up before any deliberate identity cleanup.

## 4. Export approved data

```sh
cargo run --bin wrkgn -- export
```

Export includes approved public records across the database, with corrections and suppression applied. It writes a completed versioned snapshot and atomically updates `local-data/public-export/current.json`. Raw payloads, private review history, credentials, and SQLite are excluded.

`process` and mutating review commands also attempt export automatically. `review pending` does not. An export failure can happen after processing or a review mutation has already committed, so inspect the current state rather than assuming the preceding work was rolled back.

If export reports missing project excerpts or legacy audience labels, reprocess the affected threads with the current extractor and review the new results. Database migrations preserve legacy records; they do not silently relabel or approve them. For older free-text **category** data, the explicit offline migration is:

```sh
cargo run --bin wrkgn -- categories normalize
```

Unknown category labels abort normalization before any write. This command does not fix legacy audiences or missing excerpts.

## 5. Stage and inspect the website

After review and a successful export:

```sh
cd website
npm run stage -- ../local-data/public-export
npm test
npm run build
npm run dev
```

Staging validates the selected public snapshot and replaces `website/data/public/`, removing previous staged versions. You do not need to delete old staged data first. Failed validation leaves the staged snapshot intact. Dev and build commands consume only staged data; they never collect HN data, call the model, or approve records.

Comment counts for sorting are exported from stored reply relationships. Run `export` and stage again to update counts after collection and review; no model rerun is needed solely to add counts.

Inspect list cards, all four date/comment sort orders, direct project URLs, source excerpts and attribution, Category/Audience filtering, ordinary pagination, and modal back/forward behavior. Check narrow and wide layouts. See the [manual browser checklist](docs/development.md#manual-browser-checklist). Stop the dev server with Ctrl-C.

The shared detail component shows the story/comment posting author separately from the actual author and date of each supporting evidence entry. Builder attribution remains an assumption based on the source author and explicitly named additional builders, not verified ownership.

## 6. Publish through the existing Cloudflare integration

From the repository root, inspect `git status` and the diff, including changes under `website/data/public/`. Commit the selected approved snapshot and any intended source changes, then push through the repository workflow used by the existing Cloudflare integration. Local export or staging alone does not update wrkgn.com.

Cloudflare builds the committed website using `website/` as the build root, `npm run build` as the build command, and `dist/` as the output directory. Confirm deployment success in Cloudflare and inspect https://wrkgn.com afterward. No new hosting integration or manual database upload is needed. Keep a working correction/removal contact route on the public site.

## Checks after code changes

For Rust changes, run from the repository root:

```sh
cargo fmt --check
cargo clippy --all-targets
cargo test
```

For website changes, run `npm test` and `npm run build` from `website/`, plus the relevant browser checks. Ordinary automated tests use offline fixtures; live model checks are explicit `process` runs.
