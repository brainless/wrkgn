# Epic 05 implementation record

Epic 05 was implemented by six agents, one task at a time, as requested. No review pass was performed. The maintainer subsequently tested manually and reported that it looks good. Filtered projects appeared in different positions compared with the Astro app in `website/`; this ordering parity issue is explicitly deferred, with its cause uninvestigated. Astro was the production site until Epic 06 and has since been removed; no deployment, staging, approved-data refresh, extraction, or model calls were performed for this work.

| Task | Delivered |
| --- | --- |
| 1 | Cargo workspace; shared public snapshot types/schema/validation; schema 6 atomic SQLite + JSON export; Astro schema compatibility |
| 2 | In-memory discovery, four presorted orders, Unicode substring search, filters, pagination and two-stage canonical redirects; shared URL fixtures |
| 3 | Axum/Tokio server/CLI, read-only blocking connection pool, SIGHUP snapshot swap, routes/sitemaps, cache/security headers, weak ETags, gzip, load shedding and path-only logging |
| 4 | Askama layout/list/card/detail/control templates, existing CSS/design, attribution/evidence rendering and embedded hashed asset build |
| 5 | TypeScript discovery navigation/history, shared canonicalization, mobile filter overlay and accessible project modal |
| 6 | Operational/product/agent documentation; maintainer-run semantic/content parity, synthetic public snapshot generator, HTTP performance and retained-snapshot memory tools |

The root pipeline package, existing binaries, pinned nightly toolchain and development/test Cranelift settings remain. The server depends on the shared snapshot crate rather than pipeline/Rig/HN code. Release builds retain LLVM. JSON generation/staging and the Cloudflare repository publication were kept for Astro only until the 2026-11-18 cleanup removed them.

## Checks actually reported

The task agents reported these checks passing during implementation:

- Snapshot: `cargo fmt --all`; `env RUSTC_WRAPPER= cargo check --workspace --all-targets --offline`.
- Discovery and server: `cargo fmt --all`; `env RUSTC_WRAPPER= cargo check -p wrkgn-web --all-targets --offline`.
- Templates: `npm run build --prefix crates/wrkgn-web/assets`; `cargo fmt --all`; `env RUSTC_WRAPPER= cargo check -p wrkgn-web --all-targets --offline`.
- TypeScript enhancement: asset build and offline `cargo check -p wrkgn-web --all-targets`.

The documentation/tooling task also passed `cargo fmt --all`, `env RUSTC_WRAPPER= cargo check -p wrkgn-web --all-targets --offline` (including new generator/memory examples), `node --check crates/wrkgn-web/tools/discovery-parity.mjs` (since removed), `node --check crates/wrkgn-web/tools/performance.mjs`, and Python AST parsing of `content-parity.py`. These were syntax/compilation checks only; the tools themselves were not executed. These checks establish buildability only. They do not establish route correctness, browser behavior, publication safety, semantic/content parity, or performance.

## Maintainer validation pending

Build the assets first, then run the documented workspace formatting, Clippy and test commands. Execute the website tests/static build and TypeScript shared canonicalization tests. Offline test definitions were added but not executed during this implementation.

Start with a deliberate schema-6 export, run `wrkgn-web check`, then inspect locally. Follow the [development checklist](../development.md#manual-browser-checklist) with JavaScript enabled/disabled, wide/narrow layouts, canonical redirects, history, modal failure/version fallbacks, focus restoration and CSP console checks. Check HTTP headers/statuses including HEAD/405, 301/302, 404, 304 and 503. Validate both successful/invalid reloads while requests are in flight, and confirm old data remains on rejection. Inspect schema/content against the public allowlist and run unchanged export twice to verify stable version selection.

The semantic/content parity tools were removed with Astro. Run the full and synthetic 100,000-project performance commands and memory probes. Record hardware/build/snapshot identity, p99 latency, one/two-snapshot resident memory and sampled reload peak here. The target is p99 below 50 ms on both datasets; no measurement has yet established this. FTS remains conditional on a measured failure of that budget. Epic 06 service limits must use actual measured memory plus headroom.

No full Rust tests, Clippy, website tests/build, parity scripts, performance tools, live services, browser automation or manual browser interactions were run by this task sequence. A real correction/removal destination and previous extraction-quality pilot checks remain maintainer responsibilities.
