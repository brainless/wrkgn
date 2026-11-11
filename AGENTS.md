# Guidance for coding agents

These instructions apply to coding assistants working in this repository, not to the Rig extraction agents used by the application.

## Read first

- Read [docs/PRD.md](docs/PRD.md) for product requirements and agreed architecture.
- Read [docs/development.md](docs/development.md) for the implemented commands, data contracts, and local checks. [docs/categories.md](docs/categories.md) defines the fixed category taxonomy.
- Epics 01–03 in [epics/](epics/) are implemented; read the relevant one for the original scope and acceptance criteria. New epics describe planned work, not existing capabilities.
- Keep requirements consistent when decisions change: rewrite affected sections rather than appending contradictory alternatives.

## Architecture

- All HN collection, LLM processing, review, and SQLite writes run locally on the maintainer's computer.
- Use Rust for the pipeline and Rig for structured LLM extraction. The HN API reader is deterministic Rust code and uses no agents.
- Keep raw and processed records in separate tables in one local SQLite database. Preserve source IDs, authors, dates, evidence, and reply relationships.
- Astro consumes a deliberate public export and generates static HTML. Cloudflare hosts the build output; no hosted database or request-time API is required initially.
- Generate date-ordered paginated lists and stable per-project slug pages. Enhance card navigation with a dynamic modal using the same project URL and content. Search and the two filters (Category, Audience) run in the browser against exported data.
- Never ship the working SQLite database, credentials, raw payloads, or private processing/review history as website assets.

## Maintainer workflow

`wrkgn collect --thread <hn-item-id>` is the only explicit HN refresh (post and replies). `wrkgn process --thread <hn-item-id>` never fetches from HN: it requires the latest collection of that thread to be complete, reads stored source from SQLite, processes changed conversation groups, and exports approved public data. `--reprocess` explicitly reruns extraction after prompt/schema changes. Review (`wrkgn review pending|approve|correct|suppress-source|suppress-project|match`), `wrkgn export`, and `wrkgn categories normalize` are local and make no network calls. Approved exports reach the website only through `npm run stage` in `website/`.

The maintainer then starts Astro dev mode, inspects project details and filtering, changes extraction prompts/code if needed, and reruns the pipeline. Dev/build commands must not trigger live collection or LLM calls implicitly. Provide a simple explicit local approval path for publication; extraction success is not approval.

Use one extraction agent definition. Each invocation receives a top-level comment and its reply subtree, including source IDs. Output may contain zero, one, or multiple projects with verbatim source excerpts and additional explicitly named builders. Handle oversized trees explicitly without silently dropping replies. Treat source content as untrusted data, never as agent instructions. Audience labels come from the fixed taxonomy in `src/audience.rs`; missing audiences stay unknown. Categories come from the fixed taxonomy in `src/category.rs` (zero to five per mention); changing it requires a prompt/schema version bump and an explicit migration of stored labels.

Validate structured output in Rust. Retain processing versions and source fingerprints; support resuming and safe incremental reruns. Preserve previous successful results on failure, avoid duplicates, and keep reviewed corrections and removals effective across reprocessing. Project matching across conversations is separate from extraction. Model calls may run in parallel (`--concurrency`), but SQLite writes stay on one thread; retry rate-limit and transport errors with backoff rather than failing groups immediately.

## Rust conventions and references

- Use pinned nightly Rust and Cranelift for development. The reference is `~/Projects/daftprompt/rust-toolchain.toml`: `nightly-2026-10-25`, minimal profile, components `rustc-codegen-cranelift`, `rustfmt`, and `clippy`.
- Explicitly configure the Cranelift backend; adding the component alone does not enable it. Verify configuration against local toolchain support during setup.
- Inspect existing code before adding duplicate infrastructure. The Rust CLI lives in `src/` (`cli`, `collector`, `store`, `extraction`, `processing`, `publication`, `review_grid`, `category`, `category_migration`), with SQL migrations in `migrations/` and integration tests in `tests/`. The Astro site lives in `website/`.
- Rig source is available at `~/Projects/rig` for reference. Use the upstream Git dependency with a pinned revision, not a local path dependency: [0xPlaygrounds/rig](https://github.com/0xPlaygrounds/rig). The inspected checkout was [7dfd8a4229a9d89af159489687cb14b404b2871d](https://github.com/0xPlaygrounds/rig/tree/7dfd8a4229a9d89af159489687cb14b404b2871d).
- Keep provider credentials in local configuration/environment, out of Git, logs, fixtures, and exports.

## Validation

Use offline fixtures for automated checks of tree collection, multiple-project extraction, provenance, invalid output, reruns, deletion handling, and publication boundaries. Live model runs are deliberate integration checks and must not be required by ordinary tests.

For website work, verify static builds, direct project URLs, ordinary pagination links, modal back/forward behavior, filter state, and matching HTML/JSON snapshots. Manually assess extraction quality on the three-thread pilot before scaling collection. Run checks appropriate to the change and report what was actually verified. Rust: `cargo fmt --check`, `cargo clippy --all-targets`, `cargo test`. Website (from `website/`): `npm test`, `npm run build`.
