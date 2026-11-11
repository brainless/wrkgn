# Epic 02: Structured project extraction and public export

Status: Implemented with offline fixtures. Live model quality check and three-thread pilot deferred until thread IDs and credentials are supplied.

Depends on: [Epic 01](01-local-collection-and-storage.md)

## Outcome

The maintainer can collect a supplied thread with `wrkgn collect` and then process it with `wrkgn process`, inspect and approve results locally, and export a consistent public dataset for Astro. Prompt changes can be evaluated through deliberate reruns without losing corrections or duplicating projects.

## Scope

1. Add Rig as a pinned upstream Git dependency, using the local clone for reference. Configure provider/model credentials locally through shell variables or a Git-ignored `.env` file loaded at CLI startup. OpenAI remains the default provider with an explicit completion model; Neuralwatt uses the OpenAI-compatible chat route and defaults to `glm-5.3-flash`. Include provider, model, and endpoint in processing identity. Define a preset system prompt and typed extraction schema, with version identifiers.
2. Assemble one top-level comment and its reply subtree per extraction input, retaining IDs, authors, dates, and relationships. Define a bounded strategy for oversized trees that retains project/ancestor context and accounts for every source item. Do not silently truncate.
3. Extract zero or more mentions, names when present, explicit URLs, factual descriptions, problems/audiences, proposed category/audience labels, explicit requests with evidence, and supporting contributions. Attribute reply-originated projects and contributions to their actual source items.
4. Validate structured output in Rust before committing it. Reject invented/out-of-input source IDs and unsupported request evidence. Keep facts distinct from classifications; missing values remain unknown. Treat comments as data rather than executable instructions.
5. Add related tables for projects with stable slugs, mentions, links, labels, requests/evidence, supporting contributions, processing runs, review state, corrections, and suppression. Define stable identity on reruns; do not rely on array position or regenerated titles. Handle ambiguous mappings conservatively.
6. Separate project matching from extraction. Reuse identities only with adequate evidence; preserve uncertain matches as separate candidates for review. Use the fixed category taxonomy in `docs/categories.md`, with zero to five distinct slugs per mention; validate unknown, duplicate, and excessive values. Migrate pilot free-text labels explicitly and transactionally, dropping generic format, technology, and stage labels. Audience labels remain free text. Only explicit requests populate Looking for.
7. Complete `wrkgn process --thread <id>` through processing and export of approved records. It makes no HN calls: it reads stored source and requires the latest collection run for the thread (from `wrkgn collect`) to be complete. Add `--reprocess`, resumable run status, source/group fingerprints, and prompt/model/schema version tracking. Reply edits invalidate their affected conversation group. Failed reruns retain prior successful output; successful replacement is transactional.
8. Provide a minimal local review/approval and correction/suppression interface. Newly extracted data remains unpublished until approved. Changes to reviewed content require renewed review unless a preserved correction explicitly determines the result. Deleted or suppressed sources cannot remain public merely because extraction failed.
9. Export a versioned public snapshot: a compact discovery dataset, per-project detail JSON, and a manifest/schema version. Include only approved public fields and apply corrections/removals consistently. Export all approved projects, not only those in the most recently processed thread.

## Public data contract

- Discovery records: stable ID/slug, card fields, searchable text including supported builder usernames, fixed category slugs, Audience/Looking for values, and original latest published mention date. The versioned manifest carries category slugs, display labels, and definitions.
- Detail records: public descriptions, links, dated mentions, attributed supporting contributions, and source URLs/IDs.
- Ordering: latest published mention sharing date, with deterministic tie-breaking; collection time and supporting replies do not affect ranking.
- Build consumers use a single completed snapshot. Write exports atomically and version their assets. No raw payloads, private review records, model responses, credentials, or database files are published.

## Acceptance criteria

- Fixtures cover no project, multiple projects, unnamed projects, a project introduced in a reply, relevant and irrelevant replies, repeated mentions, and explicit versus inferred invitations.
- Every mention and supporting contribution resolves to supplied source evidence with correct attribution and sharing date.
- Invalid JSON/schema or provenance fails cleanly and cannot partially replace successful records.
- Unchanged runs skip successful groups; explicit reprocessing reruns them. Successful reruns do not duplicate records or change established slugs.
- Corrected and suppressed records remain corrected/suppressed across reruns; removed source material disappears from every affected public export.
- Oversized groups have an explicit processing record for all items, with incomplete processing reported rather than published as complete.
- Review state gates publication. A second thread's run does not remove previously approved projects from the export.
- HTML consumers and modal/search consumers can use the same documented snapshot contract without accessing SQLite or an LLM.

## Validation

Use deterministic mock model outputs for automated schema, provenance, rerun, review, suppression, and export checks. Run a deliberate live extraction on a small conversation before the three-thread pilot. Record qualitative findings about labels and extraction accuracy; do not equate schema validity with factual accuracy.

## Boundaries and handoff

This epic produces the data contract and local review workflow, not the Astro UI or Cloudflare deployment. The next epic should implement static date-ordered pagination, generated project pages, URL-aware modals, browser search/filters, and the maintainer's Astro dev inspection loop. Full cross-thread automated deduplication and a dedicated search index remain deferred until pilot evidence justifies them.
