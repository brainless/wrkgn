# Epic 04 implementation record

The requested work is divided among agents by task: shared workflow, extraction policy and execution, storage and publication, and discovery and CLI. The primary agent handles website contracts, documentation, and validation. No separate review pass is performed.

Phase 1 completed before Show HN implementation: extracted `workflow` orchestration with explicit approval policy and `ProcessingUnit`/`ExtractionPolicy` boundaries. Existing monthly versions, fingerprints and extraction input remain unchanged. Validation passed: `cargo fmt --check`, `env RUSTC_WRAPPER= cargo clippy --all-targets`, and `cargo test`. The wrapper override avoids a local sccache sandbox permission failure.

Phase 2 adds persisted modes, story-anchored extraction, multi-source evidence, bounded/resumable chunk work, and public schema 5. Phase 3 adds date-window discovery and the separate CLI only after Phase 2 offline tests pass.

Live extraction quality and browser interaction checks remain maintainer tests. Ordinary automated checks use fixtures, never live HN or model calls.

Website schema-5 contract tests and existing website tests pass. The current staged legacy snapshot builds (3353 pages). An isolated synthetic mixed schema-5 snapshot also builds; direct project HTML, matching detail JSON, list aliases, and escaped title evidence were checked. The maintainer's staged snapshot was preserved. Browser back/forward, modal interaction and live factual quality remain manual checks.

Phase 2 gate passed before binary creation: `cargo fmt --check`, `env RUSTC_WRAPPER= cargo clippy --all-targets`, and `env RUSTC_WRAPPER= cargo test --quiet`. Eleven Show HN processing fixtures pass, including chunk resume/consolidation, cross-mode matching, stable slugs, sharing dates/comment counts, stale-result publication, suppression, authorization, zero-project retirement and invalid output. Three additional storage fixtures cover migration/modes and evidence freshness.

Oversized-input limit: the initial chunk strategy repeats submitter context and required ancestors to preserve authority dependencies. It can process large third-party discussions, but explicitly fails when repeated submitter/ancestor context cannot fit 30 KB, or bounded consolidation cannot fit. No sources or intermediate evidence are silently truncated.

Final checks with the new binary registered: formatting, all-target Clippy, full Rust suite, website tests (8), legacy static build and isolated mixed-v5 static build passed. Show HN processing now has 13 fixtures and CLI/discovery has 8 unit fixtures. `env RUSTC_WRAPPER= cargo run --bin wrkgn-show-hn -- --help` passed without network/model calls. No separate code review, live collection, LLM call, website staging, commit or deployment was performed.
