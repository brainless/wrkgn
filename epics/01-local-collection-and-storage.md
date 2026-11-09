# Epic 01: Local HN collection and durable storage

Status: Implemented and validated with offline fixtures. Live smoke test deferred until a monthly discussion ID is supplied.

## Outcome

Given an HN monthly discussion item ID, the maintainer can run a local Rust CLI to collect the post and complete reply tree into SQLite, refresh it safely, and resume after failures. This establishes the source dataset for extraction without using an LLM.

## Scope

1. Inspect reusable local tooling, then establish the Rust CLI with pinned nightly and an explicitly enabled Cranelift development backend. Document setup and verification. Use the toolchain reference in AGENTS.md.
2. Add versioned SQLite migrations and configurable local database location. Store HN item identity, raw payload, parent/reply relationships, author, original timestamp, content fingerprint, collection/refresh timestamps, and deleted/dead state. Track collection runs and individual fetch failures.
3. Implement the HN API reader for a supplied thread ID. Traverse all descendants, retain ordering/context, bound concurrency, and retry transient failures. Distinguish unavailable/null responses from explicit deleted/dead records; report incomplete trees.
4. Introduce `wrkgn process --thread <id>` as the orchestration entry point and a collection-only mode or subcommand for debugging. In this epic, clearly report that extraction/export are not yet implemented; never report a complete pipeline success.
5. Make repeat collection idempotent, refresh existing items, and retain sufficient change history to invalidate derived results later. Preserve provenance without interpreting comments as projects.

## Acceptance criteria

- A fixture thread with nested replies is stored with correct item IDs, authors, original dates, and parent relationships.
- Repeating collection creates no duplicate source items; edited content changes its fingerprint and records refresh state.
- An explicit deletion is distinguishable from a timeout, missing response, or failed request.
- An interrupted or partially failed run can resume; its failure summary identifies affected items and does not claim completeness.
- No agent or model credentials are needed to collect a thread.
- Database files and local credentials are excluded from version control and future public assets.
- Documented setup demonstrates nightly and Cranelift selection; relevant formatting and automated fixture tests pass.

## Validation

Use an injectable API client or fixture transport to test nested traversal, repeat runs, edits, deletion signals, and transient failures without network access. Perform a deliberate live smoke test on a supplied discussion ID once available; do not invent a monthly thread ID.

Implemented validation: offline fixtures cover nested ordering and attribution, repeat refreshes and edited fingerprints, explicit deleted/dead records, null and failed fetches, transient retries, invalid parent/cycle detection, interrupted and partial-run recovery, and absence/reappearance in a confirmed complete tree. A version-one database upgrade and CLI exit statuses are also tested. No live HN collection has been run because no discussion ID was supplied.

## Boundaries and handoff

No extraction, taxonomy, project deduplication, Astro UI, or deployment in this epic. Provide reusable storage/query interfaces for ordered conversation groups and source changes. Epic 02 extends the same command into the full processing/export workflow.
