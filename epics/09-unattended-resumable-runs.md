# Epic 09: Daily, resumable runs

Status: Implemented (Phases 1–4); offline validation only. Decisions dated 2026-11-22 by the maintainer.

Depends on: [Epic 07](07-batched-show-hn-ranges.md) and the existing `wrkgn-monthly` workflow. Epic 07 has offline validation only.

## Outcome

`wrkgn-monthly` and `wrkgn-show-hn` are simplified for a daily or cron schedule. Each no-argument run collects a small amount of data, stops, and continues next time from where it left off. Both binaries use one shared run driver; only discovery differs. Persisted coverage and a work queue make a run resumable after any stop, including a kill. The terminal shows live status and an end-of-run report that is useful as cron mail and pasteable to a coding agent. There are no log files; SQLite holds only coverage, queue, run and grouped-error records.

## Motivation

- `wrkgn-monthly` defaults to a fixed 24-month window and ignores what the database holds. `wrkgn-show-hn` requires `--thread` or `--from/--through`.
- Neither keeps a work list. A killed run loses its plan and the maintainer must remember where to restart.
- The two CLIs grew organically and duplicate per-binary loops and summaries: monthly joins all failures; Show HN truncates messages to 200 characters via `short()` and reports only failure kinds. Neither shows the current phase or item.
- A transient provider or network failure today stops only the current thread or batch (`stop_after_transient_failures`) and the CLI keeps hammering a dead API.

## Product decisions

### Shared run driver

One driver implements: lock, interrupt handling, failure circuit breaker, coverage and queue, collection, extraction, approval policy, export, live status and end report. Discovery is the only per-command part. A unit is a UTC day for Show HN and a UTC month for monthly; items are stories and threads. The refactor removes the per-binary loops and summaries in favor of the shared report and reuses existing code: Rig extraction, `HnApi`/`HttpHnApi`, Algolia discovery in `src/monthly.rs` and `src/show_hn_discovery.rs` (including `Batches`), processing and fingerprints, approval and `publication::export`.

### No-argument run

1. **Resume**: finish units left `pending` or `in_progress` by a previous run.
2. **Head**: refresh recent units with a full collection walk. There is no change gate; head items are always walked fully. Head re-walks reprocess only changed conversation groups, using the existing per-group fingerprints.
3. **Backfill**: process the newest uncovered units older than the head, newest first, within the limits.

Then stop. The next run continues. The backfill floor is the start of HN (2007-02, or 2007-02-01 for Show HN), but each run is small and there is no unbounded default.

Head and backfill have separate budgets. Head work is bounded only by its window and does not count against the item cap, so a busy head can never starve backfill. The item cap applies to resume and backfill work together, so backfill advances on every run until the cap or the backfill-unit limit is reached. Head still runs before backfill.

Defaults are named constants overridable by flags:

| | Head (bounded by window, outside the item cap) | Backfill per run | Item cap per run (resume + backfill) |
|---|---|---|---|
| Show HN | last 3 UTC days (today and 2 previous), `--days N` | 3 uncovered days, `--backfill-days N` | 100 stories, `--max-items N` |
| Monthly | current and previous UTC month, `--months N` | 1 uncovered month, `--backfill-months N` | 5 threads, `--max-items N` |

The item cap counts resume and backfill items actually collected and processed; head items are not counted. The run stops at the next item boundary; a unit cut short stays `in_progress` or `pending` and is resumed next run. `--max-duration` is dropped; unit and item limits suffice.

### Dropped

The `descendants` change gate, root-only fetch, `--full-refresh`, `--refresh-months` (12-month window) and the Show HN 30-day/12-month re-collection rule are dropped. The head is short, so fresh items get full walks while discussion is active and are then left alone. Caveat: comments, edits and deletions arriving after an item leaves the head window are picked up only by an explicit `wrkgn collect --thread` followed by `wrkgn process --thread`.

### Coverage and queue (migration 008)

New migration 008; `LATEST_SCHEMA_VERSION` becomes 8. `tests/storage.rs` asserts the literal 7 (around lines 25, 84 and 126) and must be updated. Exact columns are for implementation; the intent is:

- **Coverage table**, keyed by command and unit (`YYYY-MM` or `YYYY-MM-DD`): state (`pending`, `in_progress`, `done`, `failed`), attempts, timestamps, last run ID. A unit is `done` only when discovery succeeded and every item in it was collected and processed successfully (approval per policy). Empty units (no discussion, no stories) are marked `done` so they are not searched again. Show HN non-qualifying stories (no `thread_extraction_modes` row) are covered by their day being `done` and are not rescanned. Head units are revisited every run while inside the head window regardless of state; once they leave it, their last state stands and an unfinished one is resumed.
- **Queue rows** under each unit for items (thread or story IDs), unique key (command, kind, key), so a run killed mid-unit resumes without rediscovery. Existing per-group fingerprints keep redoing work safe.
- **Retries**: failed units and items are retried on later runs up to a fixed attempt cap (constant, 3). After that they stay `failed` and the end report lists them as needing manual action (for example `wrkgn collect --thread`).
- **Run table**: command, timestamps, status (`completed`, `limit_reached`, `interrupted`, `stopped_on_failure`, `killed_detected`) and counts.
- **Run-error table**: run ID, normalized signature, stage, kind, count, first and last item IDs, one full sample message. Size is capped; no payloads, prompts or credentials.
- Show HN days with more stories than `--batch-size` use the existing `Batches` inclusive cursor within the day window.

**Monthly thread-to-month mapping.** A thread's month is parsed from its title when discovered or collected and persisted (a month can hold several threads). A new title-to-`Month` parser must agree with `Month::matches_title` (full and abbreviated names including "Sept", optional "Ask HN:", optional "?"); add `Month::next` and make the current month accessible. Existing monthly threads are backfilled by a one-time Rust step on the first monthly run, reading stored titles (`json_extract(raw_payload,'$.title')` of root rows with mode monthly), not in the SQL migration.

### Circuit breaker

This replaces the per-thread `stop_after_transient_failures` (`src/processing.rs`, around lines 199-209 and 405). It is a deliberate behavior change: today the threshold stops only the current thread or batch and the CLI continues.

- Separate consecutive-failure counters per API type: LLM provider, HN Firebase API and Algolia, shared atomically across all parallel workers.
- A failure is a call that exhausted its existing retries and backoff, not each attempt. Any success resets that counter.
- Three consecutive failures on any counter stop the whole run cleanly: no new calls, in-flight calls finish and are saved, approved data is exported, the run is recorded `stopped_on_failure` and the report prints.
- Not counted: model validation failures, HN null or deleted items, empty search results.
- `Collector::collect` gains a stop flag checked between fetch waves. A stopped collection ends `incomplete`, which is safe because `finish_run` reconciles `present_in_tree` only on complete runs.

### Lock

One instance per command, via a lock file next to the database holding the PID, created with `File::create_new`. Stale-PID detection follows the precedent in `src/snapshots.rs` (PID liveness for staging directories). A stale lock is taken over, the old run is marked `killed_detected` and a short note is printed. A second live invocation exits immediately with a clear message, so cron overlap is safe.

### Interrupts

The first Ctrl-C or SIGTERM sets a flag. The run stops at the next item boundary (per story or thread, not per batch); in-flight model calls finish and are saved. It exports approved data, records `interrupted`, prints the report and exits 130. A second Ctrl-C exits immediately. The root crate has no signal dependency; add one (for example `ctrlc` with the `termination` feature).

### Exit codes

- 0: completed or limit reached with no failures.
- 1: failures occurred, or a setup error (as today).
- 2: stopped on failure (circuit breaker).
- 130: interrupted.

Nonzero only when attention is needed.

### Terminal output (stderr, no log files)

1. **Live status.** One line with phase, unit, item and counts. On a TTY it is rewritten in place; otherwise plain lines at most once per item batch or 30 seconds. It composes with the existing processing `Status`.
2. **End-of-run report**, on every ending, as pasteable Markdown, in this order:
   - outcome and exit reason;
   - errors grouped by normalized signature (digits, IDs and URLs stripped, replacing `failure_kind`'s 80-character fallback) with stage, kind, count, first and last IDs and one full sample;
   - work done: units covered, items collected, processed and approved, exported snapshot;
   - what remains: pending and failed units, and what the next run will do.

   The report must be useful as cron mail. No credentials, payloads or prompts appear in it.

### Overrides and review policy

Monthly `--from/--through YYYY-MM`, Show HN `--from/--through YYYY-MM-DD` and Show HN `--thread ID` stay as overrides. They bypass head and backfill, mark coverage for the units they complete, use the queue and print the report. Review policy is unchanged: Show HN results stay pending unless `--auto-approve`; monthly approves each successfully processed thread (invoking the command authorizes it, as today). `--concurrency`, `--batch-size`, `--reprocess`, `--db` and `--export` keep their meaning.

## Scope

### Phase 1: Shared run infrastructure

Migration 008 and store methods; lock; signal handling; circuit breaker including the collector stop flag; error normalization and grouping; live status; end report; exit codes. Consider a shared test-support module for fakes (`HnApi`, discovery, model), since none exists.

Offline tests: lock rejects a second live run and takes over a stale one; killed run detected and noted; interrupt stops at an item boundary and records `interrupted`; breaker counts per API type, shares the counter across workers, resets on success and ignores validation failures, null items and empty results; stopped collection ends `incomplete`; report grouping, ordering, untruncated samples and no secrets; non-TTY status cadence; failed-unit attempt cap.

#### Phase 1 completion gate

`npm run build --prefix crates/wrkgn-web/assets`, then `cargo fmt --all --check`, `cargo clippy --workspace --all-targets`, `cargo test --workspace` pass. Commit nothing; report the result before Phase 2.

### Phase 2: Monthly on the shared driver

Title-to-month parsing and the first-run backfill of existing threads; coverage; resume, head, backfill and limits; `--from/--through` as overrides.

Offline tests with fake `HnApi`, discovery and model: title parsing including abbreviations and "Sept"; existing threads backfilled; resume after a simulated kill mid-unit; head before backfill; head items do not consume the item cap; a head window larger than the cap still lets backfill advance; backfill newest uncovered first and fills gaps; empty month marked done; the cap stops resume and backfill at an item boundary and the next run resumes; failed month retried up to the cap; discovery failure skips only that month; override marks coverage.

#### Phase 2 completion gate

Same commands as Phase 1. Commit nothing; report before Phase 3.

### Phase 3: Show HN on the shared driver

Day units, `Batches` within a day, the 100-story cap on resume and backfill (head bounded by its window), resume mid-unit, overrides.

Offline tests: resume after a simulated kill mid-day; head (3 days) before backfill; head stories do not consume the story cap; a head window with more stories than the cap still lets backfill advance; backfill newest uncovered days first and fills gaps; empty day marked done and not rescanned; a day larger than the batch size uses cursor batches; the story cap stops resume and backfill at a story boundary and the next run resumes; pending review unless `--auto-approve`; `--thread` and `--from/--through` mark coverage.

#### Phase 3 completion gate

Same commands. Commit nothing; report before Phase 4.

### Phase 4: Documentation

Update `MANAGE.md`, `docs/development.md`, the maintainer-workflow paragraph in `AGENTS.md`, `docs/PRD.md` where it describes these commands, and the `--help` text of both binaries, including the post-head-window caveat and a cron example. Rewrite affected sentences rather than appending contradictions. Mark this epic Implemented with a short validation note.

## Acceptance criteria

- No-argument runs of both commands resume, refresh the head with full walks bounded only by the head window, and resume and backfill newest uncovered units first within the item cap and backfill-unit defaults, then stop; head items never count against the cap, so backfill advances every run, and the next run continues.
- Killing a run and restarting finishes pending items first without duplicating work; empty and done units are not searched again.
- Three consecutive failed calls on one API type stop the run cleanly with exit code 2, export approved data and print the report.
- A second live run of a command exits immediately; a stale lock is taken over and the old run reported `killed_detected`.
- Ctrl-C or SIGTERM stops at the next item boundary, exports, reports and exits 130.
- Every ending prints a report with errors grouped by signature and full samples.
- Failed units retry up to the attempt cap and are then listed for manual action.
- Monthly `--from/--through`, Show HN `--from/--through` and Show HN `--thread` behave as before apart from bypassing head and backfill, coverage, queue use and the report.
- No log files are written; `LATEST_SCHEMA_VERSION` is 8 and storage tests are updated.

## Validation

Offline fixtures for automated checks. Before cron, the maintainer runs a short live pilot: one default run of each command, an interrupt and resume, a second run showing continuation, and a review of one end report, consistent with the pilot rule in `AGENTS.md`.

Implementation note: offline fixture and fake tests in `tests/run_*.rs` (with `tests/common/`) cover the lock, breaker, report, monthly and Show HN drivers. Gates passed: asset build, `cargo fmt`, `cargo clippy` (only the existing `too_many_arguments` warning at `crates/wrkgn-web/src/server.rs:118`) and `cargo test`. No live collection or LLM calls ran during implementation; the live pilot remains pending before cron.

Implementation decisions: Show HN has no seed of Epic 07 data, so backfill may re-collect those days; `--thread` is item-level and changes no day coverage; monthly `--from` alone keeps its earlier 24-month meaning; a discovery failure fails the whole unit with no partial queue; exhausted units keep exit code 1 until resolved.

## Boundaries

No changes to the public snapshot schema, web server, hosting scripts, extraction prompts or schemas, cache identities or processing versions. No VPS activity. No log files. Credentials, raw payloads and prompts never appear in reports, stored errors or terminal output.
