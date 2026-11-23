# Managing wrkgn.com

Epic 06 provides the VPS deployment and data-publication commands below. The maintainer completed live VPS setup and staging verification, and cut `wrkgn.com` over to the tunnel on 2026-11-18. The legacy Astro site and Cloudflare Pages path have been removed; production is served only by the Rust `wrkgn-web` server. Rollback is by `scripts/deploy.sh --rollback` and `scripts/publish-data.sh --rollback`, described in [hosting](docs/hosting.md).

Collection, model processing, review, the working database, and export stay on the maintainer's computer. The VPS receives only a release binary and the allowlisted public SQLite snapshot. The new publication flow is:

```text
Code: commit → scripts/deploy.sh
Data: collect → process → review / approve → export → inspect locally → scripts/publish-data.sh
Check: scripts/status.sh
```

Data publication does not use Git or a Cloudflare build. The export contains only the SQLite snapshot, its manifest and the version pointer, so snapshots impose no repository size or file-count constraint.

## Automated monthly workflow

The separate `wrkgn-monthly` binary reuses the existing collector, incremental processor, project matching, approval state, and exporter. The original `wrkgn` binary retains its manual review workflow.

```sh
# Daily default: resume, refresh the head, backfill a little, stop
cargo run --bin wrkgn-monthly --
# Tune the head window, backfill size and thread cap:
cargo run --bin wrkgn-monthly -- --months 2 --backfill-months 1 --max-items 5
# Choose an inclusive range, newest month first:
cargo run --bin wrkgn-monthly -- --from 2026-11 --through 2026-09
# Refresh just one month:
cargo run --bin wrkgn-monthly -- --from 2026-11 --through 2026-11 --concurrency 3
```

A no-argument run is small and resumable, intended for a daily schedule. It proceeds in order: resume months and threads a previous run left pending, in progress or failed; re-walk the head, the current and previous UTC months (`--months`, default 2), completely; then backfill the newest uncovered months older than the head (`--backfill-months`, default 1), newest first, back to 2007-02, and stop. `--max-items` (default 5) caps threads handled by resume and backfill together; head threads never count against it, so backfill advances every run. The cap stops the run at a thread boundary and the next run continues. Coverage, the thread queue and run records live in the SQLite database; no log files are written. On first use, months of threads already stored are recorded from their titles and not searched again.

`--from` and `--through` override all of this. They process exactly that inclusive range, newest first, re-walking months already done, and cannot be combined with `--months`, `--backfill-months` or `--max-items`. `--from` alone covers 24 months ending there; `--through` alone covers the current month back to it.

Discovery searches the [HN Algolia index](https://hn.algolia.com/api) for monthly “What are you working on” story titles, accepting full and abbreviated month names (`November`, `Nov`, and `September`/`Sep`/`Sept`), optional `Ask HN:` prefixes, and question marks. It searches full and abbreviated names separately, checks every search page, and excludes unrelated titles. Estimated search hit counts are accepted; a result count exceeding the available pagination window fails explicitly. Every matching thread in a month is handled individually, in descending item-ID order, before proceeding to the previous month. A month without matches is recorded done and not searched again. Discovery depends on Algolia indexing; a month stays covered once recorded, so rerun an explicit `--from M --through M` for threads indexed later.

Each thread refreshes the full reply tree to save new comments and detect edits, deletions, and new replies. Existing fingerprint/version checks skip unchanged successful conversation groups. After the whole thread processes successfully, its pending mentions are automatically approved, including previously pending results for that thread. Pending mentions in other threads remain pending. Corrections, matching, and suppression retain their existing behavior. The exporter writes all approved public records across the database after each thread, retaining earlier threads in the snapshot.

Failures are collected rather than logged as they occur. Every ending prints a Markdown report on stderr (a live status line precedes it) with errors grouped by normalized signature with full samples, the work done, and what remains. A failed thread or month is retried on later runs up to 3 attempts, then listed for manual action: `wrkgn collect --thread <id>` followed by `wrkgn process --thread <id>`. A discovery failure fails only that month. Three consecutive exhausted calls to one API (LLM provider, HN or Algolia) stop the whole run cleanly. Exit codes: 0 completed or cap reached without failures; 1 failures or a setup error; 2 stopped by that circuit breaker; 130 interrupted. Ctrl-C or SIGTERM stops at the next thread boundary, exports, reports and exits 130; a second Ctrl-C exits immediately. The report and queue make a rerun safe after any stop, including a kill. Successful groups stay saved. Incomplete processing does not auto-approve that thread, but attempts export using existing approvals and freshness checks to remove stale public results. Approval and export are separate commits: if export fails after approval, rerunning retries export. The command uses the same `.env`, `--db`, `--export`, and `--concurrency` settings as `wrkgn`. It never deploys or publishes. After it succeeds, inspect the export locally, then publish with `scripts/publish-data.sh`.

Comments, edits or deletions that arrive after a thread leaves the head window are picked up only by an explicit `wrkgn collect --thread <id>` plus `wrkgn process --thread <id>`, or by `wrkgn-monthly --from M --through M`.

### Unattended runs

Each command holds a lock file next to the database, `<db>.monthly.lock` or `<db>.show_hn.lock` (by default `local-data/wrkgn.sqlite.monthly.lock`), containing the process ID. A second live run of the same command exits immediately with code 1, so overlapping cron runs are safe. A stale lock left by a killed run is taken over, and the old run is recorded as `killed_detected`. The two commands have separate locks but share SQLite; stagger them. Cron starts in the home directory, while `.env` and the default `local-data/` paths are relative to the working directory, so the entry must `cd` to the repository. Build once (asset build prerequisite in section 5):

```sh
cargo build --release --bin wrkgn-monthly --bin wrkgn-show-hn
```

```text
# m h dom mon dow
MAILTO=you@example.com
17 3 * * * cd /path/to/wrkgn && ./target/release/wrkgn-monthly
37 3 * * * cd /path/to/wrkgn && ./target/release/wrkgn-show-hn
```

- Credentials come from the repository `.env`; existing environment variables take precedence.
- All output goes to stderr, so cron mails every run. Exit codes 1, 2 and 130 flag problems; a wrapper script can mail only on a nonzero exit.
- Show HN results stay pending for `wrkgn review pending` unless `--auto-approve` is added.
- Publishing with `scripts/publish-data.sh` is separate and not part of cron.
- Run the live pilot described in the Show HN section before scheduling.

## Show HN workflow

The separate `wrkgn-show-hn` command refreshes a story and its entire discussion, extracts its primary project, and exports approved data. With no arguments it is a small resumable daily run; otherwise choose one story or an inclusive UTC date range (earlier `--from`, later `--through`, both required):

```sh
# Daily default: resume, refresh the last 3 days, backfill 3 older days, stop
cargo run --bin wrkgn-show-hn --
cargo run --bin wrkgn-show-hn -- --days 3 --backfill-days 3 --max-items 100
cargo run --bin wrkgn-show-hn -- --thread <hn-item-id>
cargo run --bin wrkgn-show-hn -- --from 2026-11-01 --through 2026-11-07
# Deliberately authorize story-scoped approval after successful processing:
cargo run --bin wrkgn-show-hn -- --from 2026-11-01 --through 2026-11-07 --auto-approve
# Smaller discovery batches and more parallel model calls:
cargo run --bin wrkgn-show-hn -- --from 2026-11-01 --through 2026-11-07 --batch-size 50 --concurrency 5
```

The default run works in UTC day units: resume days left pending, in progress or failed; re-walk the head (`--days`, default 3) completely; backfill the newest uncovered days older than the head (`--backfill-days`, default 3), back to 2007-02-01; then stop. `--max-items` (default 100) caps stories handled by resume and backfill together and never counts head stories; the run stops at a story boundary and the next run resumes the cut day from its queued stories. Empty days are recorded done. Unlike the monthly command, there is no seed from earlier data: days processed in Epic 07 are not recorded as covered, so backfill may collect them again, and unchanged extraction is skipped.

Results await manual review by default. Use the existing `wrkgn review pending`, correction, approval, suppression and matching commands. `--auto-approve` approves only pending mentions of each successfully processed story. `--db`, `--export`, `--concurrency`, provider configuration and `.env` work as with the existing CLIs; `--reprocess` deliberately reruns extraction. `--auto-approve` and `--reprocess` also work on default runs; a day cut short by the cap resumes without `--reprocess` unless it is passed again. Rerun without it to resume and reuse unchanged successful work.

Each day is discovered in batches, newest first. Each batch is one Algolia search of up to `--batch-size` stories (default 100, at most 1,000), so output starts immediately. The command collects the batch's stories one at a time, runs extraction with up to `--concurrency` model calls at once across the batch's stories, approves each successful story (only with `--auto-approve`), and exports once before searching for the next older batch. `--batch-size` is rejected with `--thread`. A failed story does not stop the run; failures appear in the end report on stderr, grouped by signature with full samples. A discovery failure fails only that day, which is retried on later runs up to 3 attempts and then listed for manual action. Exit codes and interrupt behavior match `wrkgn-monthly`: 0, 1, 2 (breaker), 130. Completed batches stay saved and exported, and rerunning resumes, skipping unchanged successful extraction. `--from/--through` re-walks the days it names and bypasses head, backfill and limits. `--thread` handles one story, changes no day coverage, and exits 1 if it fails.

Stories posted after a day leaves the head window (late comments, edits, deletions or Algolia indexing) are picked up only by `--from D --through D` over that day or by `--thread`.

Date discovery uses Algolia with a timestamp cursor rather than page numbers, so new posts cannot shift batches. The cursor is inclusive and stories already handled at the shared timestamp are skipped. If a full page holds only handled stories at one timestamp (that many stories share a second), discovery fails explicitly; rerun with a larger `--batch-size`. Index lag means a later rerun may discover additional stories. Collected HN content supplies extraction data; linked websites are not crawled. Only live stories with a `Show HN:` prefix, author and submission date qualify. The persisted extraction mode prevents monthly and Show HN workflows from interpreting the same thread differently. Ordinary `wrkgn process` dispatches from that mode and still never fetches HN.

Show HN facts use the title, URL and submitter explanations, plus additional builders explicitly identified by the submitter. Other participants provide context. The story retains its original date and submitter attribution; each evidence entry keeps its own source author/date. Source edits, deletion, suppression and lost builder authorization withhold stale results even if a rerun fails. Oversized discussions use bounded resumable chunks; incomplete work cannot publish a partial replacement. The initial strategy repeats submitter context and required ancestors to preserve builder authorization. If that context alone exceeds the 30 KB input bound, or consolidation cannot fit its bound, processing fails explicitly without truncating sources. A successful no-project result retires previous mentions while retaining identity history.

Start with a small live pilot: title/link-only story, text story, submitter clarifications, and an oversized discussion. Then run a one- or two-day range, or one default run of each unattended command; batched discovery, concurrent extraction and the resumable run driver have so far been validated only with offline fixtures. Inspect factual support and attribution before broad date discovery, scheduling or automatic approval. These quality checks and browser interactions remain maintainer tests. Publication remains an explicit step below.

## Local setup and paths

Run the Rust commands below from the repository root. `cargo run --bin wrkgn --` runs the current CLI source; if you have installed an up-to-date `wrkgn` binary, you can use `wrkgn` instead.

- Rust uses the pinned nightly toolchain and Cranelift configuration. See [development setup](docs/development.md#rust-toolchain).
- Copy `.env.example` to `.env` if needed and configure one provider's credentials locally. The CLI loads `.env` from the current directory; shell variables take precedence. Never commit credentials.
- `WRKGN_PROVIDER` defaults to `openai`, requiring `OPENAI_API_KEY` and `WRKGN_MODEL`. For Neuralwatt, set `WRKGN_PROVIDER=neuralwatt` and `NEURALWATT_API_KEY`; its default model is `glm-5.3-flash`.
- The default database is `local-data/wrkgn.sqlite`. Override it with `WRKGN_DB_PATH` or `--db <path>`.
- The default export directory is `local-data/public-export`. Override it with `WRKGN_EXPORT_PATH` or `--export <directory>` on commands that export. For a custom path, pass `--export <directory>` to the deploy/publish scripts too; these scripts default to `local-data/public-export`.
- Building the server's assets requires Node >=22.12 (see section 5).

Keep the local database: it contains source history, review decisions, corrections, suppression, and stable project identities. Back it up before deliberate manual database edits, with pipeline commands stopped. Generated exports can be replaced without resetting SQLite.

## 1. Collect a thread or refresh its source

Use the item ID from the HN discussion URL, for example `https://news.ycombinator.com/item?id=50417407`:

```sh
cargo run --bin wrkgn -- collect --thread 50417407
```

This fetches the post and its reply tree and stores them locally. Repeat the same command to refresh edits, deletions, and replies. In the manual `wrkgn` CLI, only `collect` fetches HN data. It does not extract projects or update the export.

Processing requires the latest collection run for that thread to be complete. If collection fails or is incomplete, rerun `collect` before processing. Repeat collection and processing separately for each thread, or use `wrkgn-monthly` for the automated path.

## 2. Process stored comments

```sh
cargo run --bin wrkgn -- process --thread 50417407
```

This calls the configured model using stored source, without fetching HN. One extraction agent handles a top-level comment and its reply subtree, with bounded chunks for oversized groups. Rust validates the results. New mentions are pending review; extraction success is not publication approval.

The extractor returns project descriptions, verbatim project excerpts, URLs, categories, audience labels, and explicitly named additional builders. The application assumes the source author is a builder. Missing audiences stay unknown. Requests and separate supporting contributions are outside the current extraction scope. See [categories](docs/categories.md) and [audiences](docs/audiences.md).

Successful unchanged groups are skipped. Changes to source, prompt/schema version, or provider/model identity cause affected groups to run again automatically. After an interruption or partial failure, repeat the ordinary command to resume. Previous successful results are retained on extraction failure; publication checks prevent stale source results from being republished.

HN ranking changes can reorder top-level comments without changing their content. These rank-only changes reuse successful extraction and preserve approval. Changes within a comment's reply subtree still trigger processing of that group. The collection summary's `changed` count includes ordering and raw-payload changes, so it can exceed the number of groups needing extraction.

To reduce parallel model calls (default: 10):

```sh
cargo run --bin wrkgn -- process --thread 50417407 --concurrency 3
```

Rate-limit and transport failures receive retries with backoff shared across workers, pausing new requests while the provider recovers. In `wrkgn process`, three consecutive exhausted model calls stop that thread's processing; a successful call resets the count. In the unattended commands, one breaker covers the whole run: three consecutive exhausted calls to one API stop it with exit code 2 after exporting approved data and printing the report. Inspect the reported failures and rerun once the provider issue is resolved.

If a structured response fails deserialization or local provenance validation, in monthly or Show HN mode, the extractor gets up to two correction attempts in total with the original source and validation feedback. Every correction must pass the same schema and provenance checks. Exhausted failures retain previous successful results. Private failure rows include the source ID and a bounded rejected value when available; transport errors include bounded, credential-redacted causes. Inspect them using the SQL query printed with the run ID. These diagnostics are excluded from public exports. After updating validation or retry code, rerun the original command without `--reprocess` to retry failed groups while retaining unchanged successful work.

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

The readable cards show extracted fields, labels, builders, and source context. Show HN cards include each stored evidence quotation with its source field, actual author/date, HN link, supported fields, and authorization-source link for additional builders; the submitter identifying statement appears among those quotations. Monthly cards retain their original single-comment excerpt and context. The JSON form retains mention keys for corrections and adds the same evidence records with authorization source IDs; corrections apply in both views. Pending review covers the database, not just the last processed thread. Check source attribution, excerpt accuracy, descriptions, labels, URLs, and possible duplicate projects. Manually assess representative cases from a small live pilot before scaling a workflow or enabling automatic approval.

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

Export includes approved public records across the database, with corrections and suppression applied. It writes a completed versioned snapshot directory `snapshots/<version>/` holding the immutable `public.sqlite` and a small `manifest.json`, then atomically updates `local-data/public-export/current.json`, the version pointer. `public.sqlite` contains only allowlisted public records; the working database, raw payloads, private review history, and credentials are excluded.

Each export with changed content adds a snapshot under `local-data/public-export/snapshots/`, and nothing removes old ones automatically, so the directory grows (3.1 GB by 2026-11-18). Only the version `current.json` selects is needed locally: publishing and deployment read just that one, and server rollback uses the server's own retained copies, not local ones. Reclaim space explicitly and offline, preferably after a dry run, and not while an export or `process` is running:

```sh
cargo run --bin wrkgn -- snapshots prune --dry-run
cargo run --bin wrkgn -- snapshots prune            # keeps current + 2 most recent others
cargo run --bin wrkgn -- snapshots prune --keep 0   # keeps only current
```

It also removes `.staging-*` directories left by interrupted exports when their process is no longer running, and refuses to run while one is live. Add `--export <dir>` to prune another export directory. A pruned version cannot be regenerated from the current database; to keep an older local version available for a deliberate re-publish (by pointing `current.json` at it), raise `--keep`.

`process` and mutating review commands also attempt export automatically. `review pending` does not. An export failure can happen after processing or a review mutation has already committed, so inspect the current state rather than assuming the preceding work was rolled back.

If export reports missing project excerpts or legacy audience labels, reprocess the affected threads with the current extractor and review the new results. Database migrations preserve legacy records; they do not silently relabel or approve them. For older free-text **category** data, the explicit offline migration is:

```sh
cargo run --bin wrkgn -- categories normalize
```

Unknown category labels abort normalization before any write. This command does not fix legacy audiences or missing excerpts.

## 5. Inspect the Rust server locally

From the repository root, build assets before Cargo (initial setup needs Node >=22.12):

```sh
npm ci --prefix crates/wrkgn-web/assets
npm run build --prefix crates/wrkgn-web/assets
cargo run -p wrkgn-web -- check local-data/public-export
cargo run -p wrkgn-web -- --snapshot local-data/public-export
```

Open http://127.0.0.1:3000. The export must be schema 6; run `cargo run --bin wrkgn -- export` first if the selected snapshot predates this implementation. The server reads the export directly and needs no credentials. `check` also accepts one `public.sqlite` file and uses the same validation as startup/reload.

For asset edits, run `npm run watch --prefix crates/wrkgn-web/assets` in another terminal. Restart `cargo run` after an asset rebuild so Cargo embeds the new hash/output. Snapshot updates need no rebuild: finish export, then run `kill -HUP <wrkgn-web-pid>` against the actual server process. Invalid replacements are logged and the old snapshot continues serving. Logs show method, path, status and duration, without query strings, cookies or IPs. `/healthz` shows build and selected snapshot versions; local builds use build ID `dev`.

Use `--bind`, `--public-origin`, `--connections`, `--render-limit` and cache options listed in [development.md](docs/development.md#rust-web-server-epic-05). Flags override `WRKGN_WEB_*` environment variables. Canonical links default to production even on localhost. Inspect with JavaScript disabled, then test in-place search, filters, sorting, pagination, modal history, mobile overlay, direct URLs, and CSP console output. The [manual checklist](docs/development.md#manual-browser-checklist) and [Epic 05 handoff](docs/implementation/epic05.md) list remaining checks.

## Inspecting project content

Comment counts for sorting are exported from stored reply relationships. Run `export` again to update counts after collection and review; no model rerun is needed solely to add counts.

Inspect list cards, all four date/comment sort orders, direct project URLs, source excerpts and attribution, Category/Audience filtering, ordinary pagination, and modal back/forward behavior in the local server. Check narrow and wide layouts. See the [manual browser checklist](docs/development.md#manual-browser-checklist).

The shared detail component shows the story/comment posting author separately from the actual author and date of each supporting evidence entry. Builder attribution remains an assumption based on the source author and explicitly named additional builders, not verified ownership.

## 6. Deploy code and publish data

Complete the one-time [VPS/tunnel setup](docs/hosting.md#one-time-setup) first. On the Mac, install Zig, `cargo-zigbuild`, and the VPS Rust target; see the hosting guide. Code deployment builds assets, validates the local export with a native checker, cross-compiles the Linux release using LLVM, verifies uploaded checksums, switches the release, restarts the server and checks its build/snapshot IDs.

```sh
scripts/deploy.sh
# Explicitly deploy a dirty working tree; the release ID records this:
scripts/deploy.sh --allow-dirty
# Rendering changes that should evict cached HTML:
scripts/deploy.sh --purge
scripts/deploy.sh --rollback
```

The first deploy also seeds the validated public snapshot if the server has none. Later code deploys preserve the selected server snapshot. A failed activation restores the prior release; the first deployment has no prior working release to restore. Deploys keep five release directories, including current and previous.

After an approved export and local inspection:

```sh
scripts/publish-data.sh
# For non-default paths:
scripts/publish-data.sh --export /path/to/public-export --checker /path/to/native/wrkgn-web
scripts/publish-data.sh --rollback
scripts/publish-data.sh --purge-only
scripts/status.sh
```

Publishing validates the exact selected `public.sqlite`, verifies the upload, atomically switches the snapshot pointer, reloads the service and waits for its snapshot ID. Failed activation restores the old pointer without purging. Existing versions are immutable and reused only when their checksum matches. Five snapshots are retained, including current and previous. Deployment/publication operations serialize on a server lock.

Set `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ZONE_ID` locally in the environment or `.env`. Scope the token to Cache Purge on the wrkgn.com zone. Publishing purges only the production and staging hostnames from `deploy/wrkgn.conf`, in one request. Missing tokens cause a warning and skip purging; old cached content can remain visible. A purge failure exits nonzero after the successful publication, so retry with `--purge-only`. Tokens never go to the VPS. Deployment does not purge unless `--purge` is supplied.

Production has served `https://wrkgn.com` from the VPS since 2026-11-18; inspect it after publishing (`https://next.wrkgn.com` remains a staging hostname on the same tunnel). To undo a bad change, use `scripts/publish-data.sh --rollback` for data or `scripts/deploy.sh --rollback` for the release; see [hosting](docs/hosting.md#staging-and-cutover). Keep a working correction/removal contact route on the public site. An UptimeRobot monitor checks `https://wrkgn.com/healthz`.

## Checks after code changes

For Rust changes, run from the repository root:

```sh
npm run build --prefix crates/wrkgn-web/assets
cargo fmt --all --check
cargo clippy --workspace --all-targets
cargo test --workspace
```

For server or asset changes, also run the relevant browser checks. Ordinary automated tests use offline fixtures; live model checks are explicit `process` runs.
