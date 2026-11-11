# Epic 03: Static directory and discovery experience

Status: Implemented locally. Automated contract, state, and static-build checks pass; browser interaction checks are reserved for the maintainer's manual test. A real correction/removal contact and the three-thread pilot remain launch prerequisites.

Depends on: [Epic 02](02-structured-extraction-and-public-export.md)

## Outcome

Visitors can browse and search approved projects, filter by their interests, and open attributed project details through either a shareable standalone page or a modal. The maintainer can inspect the directory locally and commit its public data for Cloudflare to build from the repository.

## Scope

1. Establish the Astro application in `website/`, with its own `package.json`, npm lockfile, and documented `npm run dev` and `npm run build` commands. The build generates static assets in `website/dist/`. Keep the Rust pipeline at the repository root. No Cloudflare CLI, deployment scripts, hosted database, or request-time API is required.
2. Integrate the versioned public export from Epic 02. Define an explicit local step to stage a selected completed snapshot into a tracked data directory under `website/`, including its pointer, manifest, discovery dataset, and project details. Document this step and keep it separate from dev/build. Only approved public export files may be staged; never copy the working database or the entire `local-data/` directory. Replace the staged snapshot so obsolete publication data is not retained as website assets.
3. Resolve the selected snapshot once per build and validate its schema and referenced files. Generate HTML and browser JSON from that same snapshot. Report missing or invalid exports clearly; support a valid empty snapshot with a useful empty state. Local dev and Cloudflare builds consume committed public data without collection, extraction, credentials, or access to local SQLite.
4. Generate the date-ordered first project list at `/`, an equivalent `/projects/` alias, and `/projects/page/2/` onward, with 30 projects per page. Keep the list heading and first results near the top of the page. Show search and filters in a fixed left sidebar shared across listing and detail pages on wide screens and in a collapsible panel above the results on narrow screens, opened by a Filters button in the primary navigation. Adapt uniform-height card columns to the available results width. Cards show supported names, descriptions up to six visible lines, supported builders, bottom-aligned discovery tags, and original sharing dates, with clear presentation for unnamed projects and missing fields. Keep audience information on project details. Preserve the export's latest published mention ordering and deterministic tie-breaking. Use ordinary links for cards and pagination so baseline browsing works without JavaScript.
5. Generate stable detail pages at `/projects/<slug>/` using exported slugs. Show dated mentions, verbatim source excerpts and builder usernames, and source/project links. Distinguish source attribution from assumed source-author builder identity and explicitly named additional builders, classifications from source statements, and summaries from quotations. Offer navigation to other published projects by the same supported builder using the documented attribution convention.
6. Enhance card links with an accessible detail modal using the same presentation and public data as the standalone page. Opening changes the URL to the project URL; closing restores list URL, search/filter/page state, scroll position, and focus. Browser back/forward restores the appropriate view. Direct visits and refreshes on project URLs render the standalone page. Preserve ordinary modified-click navigation and fall back to the detail page if modal data cannot load.
7. Add browser-side text search across exported searchable fields and exactly two multi-select filters: Category and Audience. Derive options from published data. Match any selected value within one dimension and all active dimensions together with the text query. Search the full discovery dataset, then paginate results in the existing date order. Persist query, selected filters, and interactive page in the URL; restore them on reload and back/forward. Reset to the first result page when criteria change, handle invalid/out-of-range state consistently, and provide result counts, clear controls, and no-results states. Do not generate every filter combination.
8. Provide responsive layouts and keyboard-accessible navigation and controls, including an accessible mobile filter disclosure, modal focus management, Escape-to-close, and accessible labeling. Show the Filters button and filter panel only after browser discovery data loads. Use one set of controls across viewport sizes, including after resizing. Render exported text safely and permit only appropriate web URL schemes for source and project links. Show historical invitations with their original dates rather than suggesting verified current availability.
9. Include the independence/attribution notice and a visible correction/removal contact route required by the PRD. A real contact destination must be set before public launch; until then, do not present a placeholder as a working route.

## Maintainer workflow

- Run the Rust pipeline and inspect pending results locally. Review representative mentions before approving them; extraction success is not publication approval.
- Explicitly stage the approved public snapshot for the website. A small representative fixture supports UI development before the live pilot is ready.
- From `website/`, install dependencies with `npm ci`, run `npm run dev` for inspection, and run `npm run build` to validate static output.
- Commit website source and the selected approved public data. Cloudflare uses `website/` as the build root, `npm run build` as the build command, and `dist/` as the output directory. Repository hosting setup is outside this implementation; a successful static build is the deployment handoff.

The staged snapshot contains approved pilot records. `website/tests/fixtures/representative/` and `website/tests/fixtures/empty/` contain synthetic records for offline checks. The public manifest carries the fixed category taxonomy; filtering uses stable slugs while the website displays its labels.

## Acceptance criteria

- A clean checkout with committed public data builds from `website/` with `npm ci` and `npm run build`, without Rust execution, HN/model calls, credentials, or a working database.
- Generated lists have correct ordering and ordinary pagination links. Every published project has a directly accessible standalone slug page that works without JavaScript.
- Modal and standalone content agree. Opening, closing, refresh, modified clicks, and browser back/forward preserve expected URLs and list context; keyboard users can open and dismiss the modal and regain focus.
- Search and both filters operate on the full published dataset with documented combination semantics. Shared URLs restore criteria and page state; empty results and invalid state are handled predictably.
- Every mention retains source attribution and original dates. Builder navigation uses the source author and explicitly named additional builders.
- HTML, discovery JSON, and detail JSON come from one snapshot. Updating the staged snapshot propagates corrections and removals through all generated output, including obsolete pages and JSON assets.
- Build output contains no SQLite files, credentials, raw payloads, private review/processing records, or superseded snapshots.
- The directory is usable on narrow and wide screens, with visible focus, labeled controls, and safe rendering of untrusted source-derived content.

## Validation

Use offline public-export fixtures covering empty data, multiple pages, equal dates, missing names/audiences, repeated mentions, reply-originated projects, multiple filter values, source excerpts, and additional builders. Check static output and matching HTML/JSON snapshot references. Use browser tests for modal back/forward, focus restoration, filter combinations, URL state, and ordinary-link fallbacks. Build successive snapshots to verify corrected and removed material disappears from output. Inspect generated assets for publication boundary violations.

Manually inspect responsive layouts and a representative approved sample from the live extraction run. Continue the three-thread extraction-quality pilot separately; website checks do not establish factual extraction accuracy. Record the commands and behaviors actually verified.

## Boundaries and handoff

This epic implements the static website and local inspection workflow. It does not add Cloudflare deployment tooling, extraction or matching changes, a review administration UI, server-side search, accounts, or a dedicated search index. Alphabetical lists and generated category landing pages remain deferred. Measure the pilot's exported data size and browser responsiveness before expanding collection or adding search infrastructure.
