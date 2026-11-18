# Epic 07: Batched Show HN date ranges

Status: Implemented (Phases 1 and 2). Validation: offline fixtures only (fake discovery, HN API and model); no live Algolia, HN or model run yet. The maintainer should run a one- or two-day live range before broad backfills. Ties larger than or equal to `--batch-size` at a single timestamp fail discovery explicitly, as specified; batches after the first carry one handled story per boundary, so they hold at most `--batch-size` minus one new stories.

Depends on: [Epic 04](04-show-hn-project-discovery.md).

## Outcome

`wrkgn-show-hn --from … --through …` handles long date ranges in batches. It discovers one batch of stories, processes it, approves (when authorized) and exports once, then discovers the next older batch. Output starts immediately, and model calls for different stories in a batch run in parallel up to `--concurrency`. A failed story is logged and the command continues; it reports all failures at the end and exits nonzero if any occurred, like `wrkgn-monthly`.

`--thread` behavior is unchanged.

## Motivation

The current range workflow:

- discovers every candidate in the range before any output, so a long range runs silently through hundreds of Algolia requests;
- processes stories one at a time and ignores `--concurrency` (`src/show_extraction.rs` never reads it);
- exports the full public snapshot after every story;
- stops at the first failed story.

## Product decisions

- Discovery walks backward from `--through` with a timestamp cursor, not Algolia page numbers. Page numbers shift as new stories are posted and stop at 1,000 hits.
- One batch is one search request: up to `--batch-size` hits (default 100, maximum 1,000), newest first, restricted to `[range.start, cursor]`. Title filtering, live/author/date qualification and the existing source checks stay the same.
- Ties: the next cursor is the oldest timestamp in the batch, inclusive, and story IDs already handled at that timestamp are skipped. If a full page contains only already-handled IDs at one timestamp, fail discovery explicitly rather than loop or skip stories.
- Within a batch: collect each story sequentially, run extraction for the batch's stories with up to `--concurrency` concurrent model calls across stories, then approve each successful story (only with `--auto-approve`) and export once.
- Failure handling: collection, mode-conflict (story assigned to monthly extraction), processing and approval failures are per-story. They are logged with the story ID and the command continues. A discovery failure ends traversal, because older batches cannot be located without it; already completed batches remain saved and exported. Export failure for a batch is logged and retried after the next batch; the final export result determines success. Setup, configuration, opening the database and writing CLI output can still abort.
- The command prints a final summary of failed stories (grouped by failure kind with one example, reusing `failure_summary`/`failure_kind` style) and exits nonzero if any story, export or discovery failed.
- Approval stays story-scoped. Failed or incomplete stories are never approved. Export after each batch still removes stale results for failed stories through the existing freshness checks.
- Rerunning the same command resumes: unchanged successful extraction is skipped through existing fingerprints and caches. Collection still refreshes every discovered story.

## Scope

### Phase 1: Cursor batches, continue on failure, per-batch export

1. Replace whole-range discovery for `--from/--through` with a batch iterator over the cursor described above. Remove window-splitting code only if nothing else uses it; keep `matches_title` and `DateRange`.
2. Add `--batch-size N` (positive, at most 1,000). Reject it with `--thread`.
3. Process each batch's stories newest first, sequentially in this phase. Split `workflow::sync_thread_checked` (or add a sibling) so callers can collect, prepare, process and approve a story without exporting, then export once per batch. Keep the existing per-thread function and its behavior for `--thread`, `wrkgn-monthly` and `wrkgn`.
4. Output, stdout unless noted:
   - `Batch N: searching stories on or before <UTC timestamp>…` before each search;
   - `Batch N: K stories (<newest UTC date> → <oldest UTC date>).`;
   - the existing per-story lines, plus `Story X: failed: <short reason>` on failure;
   - `Batch N: approved A mentions. Public snapshot: <version>` or `results await review`;
   - a final summary and the stderr extraction progress line as today.
5. Offline tests with fake discovery, HN API and model: multiple batches in newest-first order; tie handling at a batch boundary; a failing story does not stop later stories or batches and causes a nonzero result; monthly-mode conflict is a per-story failure; export runs once per batch; no approval without `--auto-approve`; `--thread` still stops on failure; empty range succeeds with no batches.

#### Phase 1 completion gate

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets`, `cargo test --workspace` pass. Commit nothing; report the result before Phase 2.

### Phase 2: Concurrent extraction across stories

1. Honor `--concurrency` across the stories of a batch. All SQLite reads/writes stay on the calling thread; worker threads perform only model calls. Reuse the existing provider cooldown, transport retry and validation correction policies from `processing`, sharing one cooldown across workers.
2. Show HN processing (`show_extraction::process` and `run_cached`) interleaves cache reads/writes with model calls and may need multiple rounds (chunks, then consolidation). Restructure it so each story exposes the model requests it needs next, results are validated and written on the calling thread, and the story advances until done or failed. Avoid a generic framework; a per-story state machine driven by a batch scheduler is sufficient.
3. Prompt, schema and cache identities, chunk bounds, coverage checks, suppression/authorization invalidation and partial-result protections must not change. A code-only change must not invalidate cached or successful work.
4. `--thread` and `wrkgn process` for Show HN threads use the same code path with one story.
5. Offline tests: results and cache contents are identical to sequential processing for a fixture batch; concurrency limit is respected (fake model counts concurrent calls); one story's failure does not affect others; multi-chunk stories still consolidate correctly.

If Phase 2 requires changing persisted identities, stored formats or validation rules, stop and report the proposed change instead of implementing it.

## Documentation

Update `MANAGE.md` (Show HN workflow), `docs/development.md`, the Show HN paragraph in `AGENTS.md`, and `docs/PRD.md` if it describes range behavior, so they describe batches, `--batch-size`, `--concurrency`, continue-on-failure and the final summary. Rewrite affected sentences rather than appending contradictions. Mark this epic Implemented with a short validation note.

## Acceptance criteria

- A long range prints its first batch line within one search request and processes stories before discovering the whole range.
- A story failure is logged, later stories and batches still run, and the command exits nonzero with a summary.
- Export runs once per batch, not once per story.
- With `--concurrency N`, up to N model calls run at once across a batch (Phase 2).
- `--thread`, `wrkgn-monthly` and `wrkgn` behave as before.

## Validation

Offline fixtures only. The maintainer runs a short live range (one or two days) deliberately before broad backfills.

## Boundaries

No changes to the public snapshot schema, web server, hosting scripts, extraction prompts, or monthly workflow behavior. No collection skipping or HN fetch caching.
