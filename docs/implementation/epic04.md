# Epic 04 implementation record

The initial implementation was divided among agents by task: shared workflow, extraction policy and execution, storage and publication, and discovery and CLI. The primary agent handled website contracts, documentation, and validation. A subsequent review identified four follow-up fixes, delegated sequentially and reviewed individually.

Phase 1 completed before Show HN implementation: extracted `workflow` orchestration with explicit approval policy and `ProcessingUnit`/`ExtractionPolicy` boundaries. Existing monthly versions, fingerprints and extraction input remain unchanged. Validation passed: `cargo fmt --check`, `env RUSTC_WRAPPER= cargo clippy --all-targets`, and `cargo test`. The wrapper override avoids a local sccache sandbox permission failure.

Phase 2 adds persisted modes, story-anchored extraction, multi-source evidence, bounded/resumable chunk work, and public schema 5. Phase 3 adds date-window discovery and the separate CLI only after Phase 2 offline tests pass.

Live extraction quality and browser interaction checks remain maintainer tests. Ordinary automated checks use fixtures, never live HN or model calls.

Website schema-5 contract tests and existing website tests pass. The current staged legacy snapshot builds (3353 pages). An isolated synthetic mixed schema-5 snapshot also builds; direct project HTML, matching detail JSON, list aliases, and escaped title evidence were checked. The maintainer's staged snapshot was preserved. Browser back/forward, modal interaction and live factual quality remain manual checks.

Phase 2 gate passed before binary creation: `cargo fmt --check`, `env RUSTC_WRAPPER= cargo clippy --all-targets`, and `env RUSTC_WRAPPER= cargo test --quiet`. Eleven Show HN processing fixtures pass, including chunk resume/consolidation, cross-mode matching, stable slugs, sharing dates/comment counts, stale-result publication, suppression, authorization, zero-project retirement and invalid output. Three additional storage fixtures cover migration/modes and evidence freshness.

Oversized-input limit: the initial chunk strategy repeats submitter context and required ancestors to preserve authority dependencies. It can process large third-party discussions, but explicitly fails when repeated submitter/ancestor context cannot fit 30 KB, or bounded consolidation cannot fit. No sources or intermediate evidence are silently truncated.

Initial handoff checks with the new binary registered: formatting, all-target Clippy, full Rust suite, website tests (8), legacy static build and isolated mixed-v5 static build passed. That handoff had 13 Show HN processing fixtures and 8 CLI/discovery unit fixtures. `env RUSTC_WRAPPER= cargo run --bin wrkgn-show-hn -- --help` passed without network/model calls. Live collection, LLM calls, website staging, commits and deployment were outside that validation.

## Follow-up review fixes

The four findings were fixed by separate agents, one at a time, with each change reviewed before the next assignment. Show HN URL validation now uses complete URL tokens in both evidence excerpts and their original source fields, preserving shared-hosting paths. Per-chunk cache keys track their actual dependencies while final freshness retains the whole thread. Every consolidation level receives bounded evidence-source author/date metadata and submitter identity. Pending-review cards and JSON expose supporting evidence and authorization statements before approval, while monthly cards retain their legacy excerpt and context.

Show HN prompt and schema versions are now `show-hn-2`, and chunk cache identities use `show-chunks-2`; the next explicit processing invocation reevaluates prior Show HN successes. Monthly prompt/schema versions remain unchanged. No storage migration or public schema change is required.

Combined validation passed: `cargo fmt --check`, `env RUSTC_WRAPPER= cargo clippy --all-targets`, `env RUSTC_WRAPPER= cargo test --quiet` (109 tests, including 22 Show HN processing fixtures), and `git diff --check`. The new fixtures cover URL truncation, independent chunk reuse, context/suppression invalidation, consolidation metadata at every level, total prompt bounds, and local review evidence in cards and JSON. Deterministic mocks verify metadata and policy plumbing; live model factual quality and browser interactions remain maintainer checks. These fixes do not change website code or staged data.
