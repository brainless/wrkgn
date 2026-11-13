# Epic 04: Shared pipeline refactor and Show HN project discovery

Status: Implemented with offline validation. The maintainer live pilot and manual browser checks remain pending. See [implementation record](../docs/implementation/epic04.md) for phase gates, checks, and bounded-context limitations.

Depends on: [Epic 01](01-local-collection-and-storage.md), [Epic 02](02-structured-extraction-and-public-export.md), and [Epic 03](03-static-directory-and-discovery.md).

## Outcome

The maintainer can discover stories whose titles begin with `Show HN:`, collect each story and its complete comment tree, and extract its primary project with supporting details from the discussion. Approved results join the existing project directory with stable identities, accurate attribution, source evidence, and original sharing dates.

First refactor the existing pipeline into reusable components while preserving current behavior. Complete and validate that phase before adding Show HN behavior or creating the separate `wrkgn-show-hn` binary. The new CLI must compose shared services rather than duplicate the monthly workflow.

## Review baseline

Reviewed against Epics 01–03, the PRD, maintainer/development documentation, and the current implementation. Recent changes include provider retry hardening (`1d4481e`), date/comment sorting (`82765ed`), mobile filtering (`cc669af`), author-safe URL matching (`4eb8aa3`), and the separate monthly workflow (`f58686e`). The working tree also contains monthly abbreviation/pagination fixes and refreshed website data. Preserve these changes; they are not part of this epic's implementation work.

The existing public exporter emits schema version 4. The main extension points are monthly orchestration, comment-oriented processing units, single-source mention evidence, and public detail rendering. Refactor these incrementally rather than replacing the working pipeline.

## Product model

Monthly discussions and Show HN use different extraction units:

| Concern | Monthly discussion | Show HN |
| --- | --- | --- |
| Anchor | A project-bearing comment | The submitted story |
| Logical processing unit | A top-level comment and its reply subtree | The story and its complete comment tree |
| Output | Zero or more projects per conversation | Zero or one primary project per story |
| Supporting replies | Context; facts remain grounded in the attributed comment | Evidence for details about the primary project, subject to attribution rules |
| Sharing date | Original project-bearing comment date | Original story submission date |

Keep one Rig extraction implementation with explicit mode-specific instructions, input assembly, and validation. Agents receive structured source data and do not discover threads, browse linked websites, operate SQLite, or approve publication.

## Phase 1: Refactor shared services first

1. Inspect `src/cli.rs`, `src/monthly.rs`, `src/extraction.rs`, `src/processing.rs`, `src/publication.rs`, and existing tests. Extract only the boundaries needed to support both workflows; avoid a generic plugin framework or unrelated rewrites.
2. Move reusable collect/process/optional-approve/export orchestration out of the monthly-specific module. Make approval policy explicit. Keep month discovery and monthly traversal in the monthly workflow, and keep argument parsing and terminal presentation at CLI boundaries.
3. Separate processing-unit assembly and extraction policy from the shared model execution machinery. Preserve shared provider configuration, bounded concurrency, validation correction attempts, rate-limit cooldowns, transport retries, failure diagnostics, and serialized SQLite writes. Define a processing-unit contract that can later accept a story anchor and supporting sources without assuming every root is a comment.
4. Preserve existing entry points and semantics: `wrkgn collect` refreshes HN; `wrkgn process` reads stored source and requires a complete latest collection; manual extraction remains pending until approved. `wrkgn-monthly` retains its date traversal, discovery matching, automatic approval after each successful thread, failure handling, and resume behavior. Preserve the current abbreviated-month and pagination handling, including changes already in progress when implementing this epic.
5. Preserve current monthly extraction rules, persisted processing identities, incremental skips, stable slugs, matching, corrections, suppression, public contracts, and exports. A code-only refactor must not invalidate successful groups or trigger model calls. Add focused regression coverage where existing tests do not exercise the extracted boundaries.
6. Update `MANAGE.md` and `docs/development.md` if implementation organization changes documented interfaces or commands. Do not describe Show HN as available during this phase.

### Phase 1 completion gate

Run `cargo fmt --check`, `cargo clippy --all-targets`, and `cargo test`. Offline regression checks must establish that both existing CLIs retain their behavior, including unchanged-run skips, thread-scoped approval, failure/resume paths, and publication boundaries. Finish this refactor as a separately reviewable change before starting Phase 2. No new Show HN binary is introduced in Phase 1.

## Phase 2: Show HN extraction and evidence

1. Introduce a persisted extraction mode for monthly-style conversations and Show HN. Backfill previously processed threads as monthly-style behavior, preserving existing keys, fingerprints, versions, approvals, and skip eligibility; unprocessed collected threads remain unassigned. Keep monthly prompt/schema versions unchanged unless their behavior actually changes. Namespace new Show HN work by mode and its own versions. `wrkgn process` dispatches using the persisted mode; for an unassigned thread it retains the existing monthly-style default. The new Show HN workflow assigns its mode after source validation and before processing. Both automated workflows reject a conflicting persisted mode before model calls or approval; no implicit conversion or duplicate interpretation is allowed. Collection alone never chooses a mode.
2. Assemble Show HN input from the stored story title, submitted URL, optional body, author, timestamp, and full comment tree, preserving IDs, authors, dates, and reply relationships. A story with no body or no comments is valid input. Include title and submitted URL in validation as first-class source fields; they must not need to appear in comment text to be usable evidence.
3. Extract the one primary project presented by the story. Return an empty result with a recorded reason if available source cannot support a project; do not invent one. Incidental projects, alternatives, competitor links, and commenters' separate projects are outside this mode's initial scope. Invalid multi-project output must fail validation rather than being silently trimmed.
4. Use submitter-authored explanations and clarifications to enrich factual project details. Other commenters' questions, speculation, praise, criticism, feature requests, or comparisons must not become established capabilities, intended audiences, or builder identities. Third-party statements alone do not establish project facts in this initial version. A submitter's answer can provide evidence, with its question retained as context. Additional builders may supply evidence only when the submitter explicitly identifies their HN username as a builder. Retain that identifying statement as an authorization dependency for their evidence; self-identification or another commenter’s claim alone is insufficient. Do not recursively extend authority through additional builders.
5. Keep story submitter attribution distinct from evidence authorship. Retain the source-author-as-builder convention as an explicit assumption, not verified ownership. Do not add discussion participants as builders automatically. Preserve existing category and audience taxonomies and unknown-value behavior.
6. Extend provenance to support multiple evidence records per mention. Each record identifies its source item, source field (such as title, URL, or body), verbatim excerpt/value, author/date through the source relationship, and the extracted field or fields it supports. Keep the story as the mention anchor even when a detail comes from a comment. Validate membership, field availability, quotation/URL support, and permitted evidence authors in Rust. Structural validation does not establish factual entailment; assess that in the pilot.
7. Version and migrate storage/public contracts deliberately. Preserve existing single-comment evidence and reviewed monthly data without forced re-extraction or invented field-level support. Introduce a new public schema for multi-source evidence, retain explicit read support for currently supported snapshots including version 4, and reject unsupported versions. Update exporter, staging validation, build readers, browser detail readers, fixtures, and shared detail rendering together. Each public evidence entry carries its source field, concise verbatim value/excerpt, actual author, original date, and source link. Render title-only evidence without manufacturing a comment excerpt. Preserve the story submitter and sharing date separately from supporting authors/dates. Escape source text and validate link schemes; do not publish whole discussions or private diagnostics.
8. Process oversized threads in bounded chunks containing the story and necessary ancestor context. Account for every source item. Validate chunk evidence, then consolidate supported details into one primary project using the same extraction implementation. Define deterministic conflict handling: prefer explicit submitter corrections when their chronology and meaning are clear; otherwise omit disputed details for review. Preserve evidence through consolidation. Bound consolidation inputs as well as extraction inputs; use validated hierarchical consolidation or fail explicitly rather than truncating intermediate evidence. Cache chunks against all their dependencies, including repeated story/ancestor context, author authorization, suppression state, model identity, and chunking/prompt/schema versions. Record coverage and resumable chunk state; a failed chunk or consolidation cannot publish a partially completed new result. Explicitly fail inputs that cannot fit even with the bounded strategy.
9. Fingerprint the story, comments, relationships, and relevant source state. Edits, additions, confirmed removals, and deletions invalidate affected chunks and the final result; unchanged chunks may be reused. Refreshing comments never changes the story's sharing date. Failed reruns preserve previous successful records internally, while freshness checks exclude stale derived content from publication.
10. Apply source suppression and deletion to every evidence dependency, not just the story anchor. Remove dependent excerpts and derived fields from publication until a fresh valid result exists; conservatively withholding the affected mention is acceptable. Exclude suppressed material from usable extraction evidence and invalidate cached work that depends on it, so reprocessing cannot restore suppressed facts. Story suppression removes its mention. A successful zero-project result transactionally retires previous active mentions while retaining identity/review history; it is distinct from a failed extraction. Keep corrections, project suppression, identity mappings, and stable slugs effective across replacement, failure, and zero-project reruns.
11. Reuse existing author-safe, explicit-URL project matching across both modes. A monthly mention and Show HN story may join the same project only under the existing evidence rules. Different authors or ambiguous matches remain separate for review; an ambiguous match involving a suppressed candidate fails the affected unit, as it does today. Match using the story submitter as source author, never a supporting evidence author. Do not add name-only or domain-only matching.
12. Count distinct eligible comments beneath the Show HN story anchor using the existing deletion/suppression rules. Deduplicate overlapping sources across a project's mentions. Supporting comments do not create extra mentions or move the project to a later sharing date. Avoid changes to the two-filter discovery interface.

## Phase 3: Separate CLI and discovery

Create `wrkgn-show-hn` only after Phase 1 is complete and Phase 2 has offline coverage. Proposed command interface:

```sh
# Explicitly refresh, process, and export one story; results await review.
cargo run --bin wrkgn-show-hn -- --thread <hn-item-id>

# Discover and handle an inclusive UTC date range, newest story first.
cargo run --bin wrkgn-show-hn -- --from 2026-11-01 --through 2026-11-07

# Explicitly authorize automatic approval after each successful story.
cargo run --bin wrkgn-show-hn -- --from 2026-11-01 --through 2026-11-07 --auto-approve
```

- Require either one thread ID or both date endpoints; reject mixed selection, invalid/reversed ranges, and missing selection before network/model calls. Do not default to a multi-year backfill. Date ranges use calendar order (`from` is the earlier date), with an exclusive next-day UTC upper bound internally.
- Support shared `.env`, provider/model configuration, `--db`, `--export`, and `--concurrency`. Support `--reprocess` as an explicit extraction rerun; ordinary invocation refreshes HN and skips unchanged successful work. This CLI is an explicitly combined network workflow. The original `wrkgn process` remains offline with respect to HN.
- Discover candidate stories through HN Algolia search, validate the trimmed title's `Show HN:` prefix locally with case-insensitive comparison, and check the collected story before processing. Require a live story with the matching prefix and usable author/date. A directly supplied nonmatching story fails clearly. Discovery candidates whose collected title/type/date no longer qualifies are reported and skipped; deleted/dead previously processed stories invalidate publication and trigger export without model calls. Null responses and fetch failures remain incomplete collection, not confirmed deletion. Use HN API collection as the source of extraction content.
- Traverse every search page, deduplicate story IDs, and sort by submission timestamp descending with a deterministic item-ID tie-break. Split date windows when necessary to stay within search pagination limits. Use nonoverlapping half-open UTC timestamp windows, subdividing down to one second; if that window still exceeds the available pagination limit, fail explicitly. Validate collected timestamps against the requested range and globally order discovered candidates before processing; do not process in search relevance order. Estimated hit counts do not by themselves prove incomplete pagination. Document index lag and that a later rerun may discover additional stories.
- Finish collection, extraction, optional approval, and export for one story before starting the next. Stop on failure with nonzero status and actionable resume information; preserve completed work. Incomplete extraction must not approve that story. Attempt freshness-aware export after processing failure as in the existing automated workflow.
- Default to pending review. `--auto-approve` authorizes approval of that successfully processed story's pending mentions, including existing pending mentions for that story; it never approves unrelated threads. Preserve corrections and suppression. Export all approved public records across the database after each story. Approval/export remain separate commits with safe retry behavior.
- Never stage website data, commit, push, or deploy implicitly. Keep existing review commands as the correction, approval, suppression, and matching interface.

## Acceptance criteria

- The shared refactor is completed and validated before the new extraction mode or binary is added; existing manual and monthly behavior remains covered by regression tests.
- Offline fixtures cover title/link-only stories, story text, zero comments, submitter clarifications, explicitly named additional builders, unrelated discussion, competitor links, uncertain facts, no-project output, malicious source instructions, and oversized threads.
- Each supported story yields at most one anchored mention. Repeated discussion references do not create duplicates. Names, links, summaries, labels, builders, and excerpts obey the evidence rules; unsupported fields remain unknown.
- Invalid source IDs, mismatched source fields, unsupported URLs, invented quotations, unauthorized evidence authors, invalid classifications, and multi-project responses cannot partially replace successful records.
- Offline tests cover legacy cache preservation, persisted-mode dispatch and conflicts, authorization-evidence removal, suppression-aware chunk reuse, zero-project retirement, and bounded consolidation.
- Interrupted and repeated runs resume safely. Unchanged inputs skip extraction; mode/version/source changes invalidate the correct work. Chunk/consolidation failures preserve prior results and cannot publish incomplete replacements.
- Supporting-comment edits, deletions, and suppression cannot leave stale facts or quotations public, even after model failure. Corrections, project suppression, reviewed matching, and slugs survive reruns.
- Existing databases upgrade without relabeling or reapproving monthly records. Old supported public snapshots remain readable; new snapshots are validated by staging and all website consumers.
- Cross-mode matching follows the existing author/URL rules. Sharing dates and deduplicated comment counts remain correct across multiple mentions.
- Discovery fixtures cover exact prefix checks, date boundaries, ordering, duplicate hits, multiple pages, estimated counts, partitioned windows, irreducible search limits, index-lag reruns, and failures.
- Manual review is the new CLI default. Explicit automatic approval is story-scoped and runs only after successful complete processing. All exports retain the existing public allowlist boundary.
- Standalone pages and modals agree on project details and evidence attribution. Static builds, pagination, filters, URL state, and browser back/forward continue to work for mixed monthly and Show HN data.

## Validation and rollout

Use deterministic API fixtures and mock model responses for ordinary automated checks. Run `cargo fmt --check`, `cargo clippy --all-targets`, and `cargo test` for each Rust implementation phase. For public contract or website changes, run `npm test` and `npm run build` from `website/`, then perform the repository's manual browser checklist with a mixed snapshot.

Before broad discovery, deliberately process a small reviewed live pilot covering a title/link-only story, a text story, a discussion with submitter clarifications, and an oversized thread. Inspect factual support, attribution, excerpts, consolidation, matching, and removal propagation. Record quality findings separately from schema validity and automated test results. Start with bounded date ranges and manual approval; opt into automatic approval after evaluating the pilot.

During implementation, rewrite affected sections of `AGENTS.md`, `docs/PRD.md`, `docs/development.md`, and `MANAGE.md` to describe the two extraction modes, evidence model, commands, approval policies, migrations, and unchanged publication workflow. Until then, this epic is the planned specification and existing operational documentation continues to describe implemented behavior.

## Boundaries and handoff

No external project-page crawling, general HN story extraction, incidental project discovery from Show HN comments, sentiment/review aggregation, requests or supporting-contribution extraction, new taxonomy, ownership verification, broader automatic identity matching, hosted database, request-time API, or deployment automation. All collection, extraction, review, and SQLite writes remain local; Astro consumes only deliberate approved public exports.
