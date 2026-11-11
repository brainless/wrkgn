# Project discovery from Hacker News

## Overview

Create a public directory of projects and products shared in Hacker News’s recurring “Ask HN: What are you working on?” discussions. Validate the workflow with three monthly threads, then expand to approximately the past two years and continue adding new discussions.

The directory should inspire people to build and share their own work, help visitors discover interesting projects, and make it easier to find builders working on related problems. It is a discovery resource, not a social network.

## Product principles

- Focus on projects and products rather than comments as standalone content.
- Organize discovery around the problems projects tackle and the people they serve. Technology choices are not the primary discovery lens.
- Extract facts faithfully and distinguish them from editorial or automated classifications.
- Requests for feedback, contributors, and collaborators are outside the current extraction and publication scope.
- Preserve attribution, source links, and useful discussion context.
- Show when information was shared so historical mentions do not imply current availability, activity, or intent.

## Audience and needs

- People seeking inspiration want to explore what others are building and the problems they have chosen to solve.
- People with a particular need want to discover projects that may address it.
- Builders want to find related work and people with shared interests.
- Potential contributors or collaborators want to find explicit invitations and follow the source to learn more or respond.

## Scope

Collect the target discussions and their replies, then identify project-bearing contributions. Publish project entries with relevant supporting context, rather than reproducing every comment as a general discussion archive.

Replies provide context and may introduce separate projects attributed to their actual source comment. Do not extract separate supporting contributions or unrelated discussion.

The initial product does not include accounts, private messaging, social feeds, follower relationships, or a separate discussion system. Visitors follow source or project links to engage with builders.

## Projects and mentions

A **project mention** is a specific instance of someone sharing or discussing a project in a source thread. A **project** may have multiple mentions across different threads and dates. One comment may describe multiple projects.

Keep each mention’s source, author, date, and evidence intact even when grouping it under a project. Group mentions only when there is adequate evidence that they describe the same project; leave uncertain matches separate for review.

For an extracted project mention, assume its source comment author is a builder. Extract additional builder or collaborator usernames only when explicitly named in that comment. This is an attribution convention, not independently verified ownership.

## Information to capture

For each project mention, capture where available:

- Project or product name and explicitly provided URLs.
- A concise, factual description of what it does.
- The problem it addresses and its intended audience, when supported by the source.
- A verbatim excerpt identifying the project and explicitly named additional builders or collaborators.
- Explicit project URLs, preferring a project origin over multiple demo paths when safe.
- Source thread, comment, author, and sharing date.

Missing information should remain unknown. A short descriptive name may be generated when no name is supplied; do not invent audiences, URLs, project status, or invitations. Summaries must remain grounded in source material, and quotations must be distinguishable from summaries.

Discovery categories and audience labels may be assigned during processing, but must be treated as classifications rather than statements made by an author. Categories use the fixed 15-value taxonomy in [categories.md](categories.md), chosen from the pilot data by project purpose. Each mention has zero to five distinct categories. The public data stores stable category slugs and publishes the associated labels and definitions. Audience labels use the fixed taxonomy in [audiences.md](audiences.md), with zero to five distinct slugs grounded in an explicitly mentioned audience. Retain the optional factual audience text separately. Looking for is not extracted or published.

## Visitor experience

The initial directory offers a paginated project list ordered by the original sharing date of each project's most recent published mention. Collection timestamps and supporting replies do not move projects to the top. Use a stable secondary ordering when dates are equal.

Visitors should be able to:

- Browse uniform-height project cards showing a concise description, original sharing date, supported builder, and discovery tags in a grid that adapts to the available results width. Keep longer descriptions to six visible lines on cards, align discovery tags at the bottom, and show audience information on project details. Set titles, labels, and controls in DM Sans and longer reading text in Source Serif 4, loaded from Google Fonts. Show 30 projects per page, keep the list heading compact, and place the first results near the top of the page.
- Search descriptions, problems, audiences, project names, and supported builder usernames.
- Narrow results with exactly two filter dimensions: Category and Audience. Options come from the published data. On wide screens, show search and the two filters vertically in a fixed, viewport-height left sidebar shared by listing and detail pages, with the logo inside it. On narrow screens, place the same controls in a collapsible panel above the results, opened by a Filters button in the primary navigation. Listing controls appear when browser discovery data loads; detail-page controls submit an ordinary form to the directory. Ordinary lists remain usable without JavaScript.
- Open a card in a dynamic modal showing project details, mentions over time, and verbatim source excerpts.
- Share or directly visit the same stable project slug URL to read a generated standalone project page.
- Follow clearly attributed links to original discussions, demos, and project websites.
- Discover other projects shared by the same builder where that relationship is supported.

Opening a modal updates the URL; closing it restores the list context. Browser back/forward navigation must work. Search, filters, and interactive pagination preserve their state in the URL. Direct project visits must work without JavaScript.

Generate the date-ordered first list page at `/`, an equivalent `/projects/` alias, later pages at `/projects/page/2/` onward, and project pages at `/projects/<slug>/`. Slugs are persisted in SQLite and remain stable across reprocessing. Alphabetical lists and generated category landing pages are deferred until there is a demonstrated need. Do not generate every filter combination.

Historical invitations retain their dates and must not be presented as verified current opportunities. Recently collected information must not be confused with recently shared information.

## Architecture and local workflow

All collection, extraction, review, and database work runs on the maintainer's computer. Rust tooling owns the data pipeline. The Astro website lives in `website/` and builds from a deliberate public export included in the repository. Local development uses `npm run dev`; `npm run build` produces static assets locally or in Cloudflare's repository build. Cloudflare builds and hosts the website without custom deployment tooling. The initial architecture has no hosted database, request-time rendering, or processing service.

The intended workflow is:

1. Update the Rig extraction prompt, schema, or processing code when needed.
2. Run `wrkgn collect` with an HN thread item ID to fetch the post and replies, then `wrkgn process` with the same ID to extract from the stored source and export that thread's data.
3. Run Astro in development mode and manually inspect project details, source attribution, and filtering.
4. Reprocess explicitly after extraction changes, repeat inspection, and explicitly approve mentions for publication. Include the selected approved public snapshot in the repository and verify `npm run build` from `website/`. Cloudflare builds the committed website and public data from the repository.

Command interface: `wrkgn collect --thread <hn-item-id>` is the only explicit HN refresh and also refreshes the thread's replies. `wrkgn process --thread <hn-item-id>` never fetches from HN; it requires the latest collection of that thread to be complete, extracts from stored source, and exports approved data, with `--reprocess` to deliberately rerun extraction. `wrkgn review` approves, corrects, suppresses, and matches mentions locally; `wrkgn export` writes the public snapshot, and `npm run stage` in `website/` selects it for the build. Incremental runs skip unchanged successful processing; interrupted runs can resume. Dev mode and builds consume exports and never invoke HN or an LLM implicitly.

### Data collection

Use ordinary Rust code, without agents, to read the supplied post and its comment tree from the HN API. Store raw source records in local SQLite, including HN item IDs, parent/reply relationships, author attribution, original dates, collection timestamps, and source refresh state. Raw and processed data live in separate tables in the same database.

Collection supports revisiting older threads, detecting changed or deleted comments, retrying transient failures, and reporting incomplete fetches. Missing or failed API responses must not be mistaken for confirmed deletions.

### Parsing and processing

Start with one Rig extraction agent definition invoked once per top-level comment and its reply subtree. Supply each source item's ID and relationship context. For subtrees too large for the model context, use bounded follow-up processing with project and ancestor context; never silently discard replies.

Use a preset system prompt and typed structured output. Each structured JSON response contains zero or more project mentions, extracted facts, classification labels, verbatim source excerpts, and additional explicitly named builders. Descriptions state project functionality directly without creator-focused narration. A reply can introduce a project of its own, so preserve the actual source of each mention rather than always attributing it to the root comment.

Rust code validates outputs before persistence, including source ID membership, required provenance, and verbatim project excerpts. Source comments are untrusted input, not instructions. The agent does not need tools or autonomous planning. Conversation groups may be extracted in parallel with bounded concurrency; provider rate limits and transport errors are retried with backoff, and persistent provider failures stop the run early so a later run can resume.

Track source content fingerprints, prompt/model/schema versions, run status, and failures. Reprocessing replaces the applicable successful output transactionally without duplicating records; failures retain the previous successful result. Reviewed corrections and suppression decisions survive reruns. Retained results affected by deleted sources still obey publication suppression rules.

Model projects, mentions, builder attribution, labels, and processing history as related records rather than one flat website table. Cross-comment and cross-thread project matching is a separate processing concern: preserve stable identities where evidence is adequate and leave uncertain matches separate for review.

### Public export and static website

Export only approved public fields from a consistent database snapshot. Raw source payloads, credentials, processing diagnostics, and private review history stay local. Approval is explicit publication state; validation alone does not imply approval. The pilot uses a local review command rather than an administrative UI.

Astro renders project pages and paginated date-ordered lists at build time, with ordinary links for crawling and navigation. Project pages and modals share their presentation and public data contract. JavaScript enhances the list with search, filters, and modals.

Publish a compact discovery dataset containing project IDs, slugs, card fields, searchable text, normalized filter values, and sharing dates. Publish per-project detail JSON for modal loading. Start with browser-side text matching and filtering; measure dataset size and responsiveness before introducing a dedicated search index.

Generate HTML and JSON from the same publication snapshot, with versioned assets to prevent mixed data versions. Rebuilds propagate corrections, removals, and refreshed information to all pages and discovery assets. Do not copy the source SQLite database into the public output.

## Freshness, corrections, and removals

- Clearly show the original sharing date for each mention and contextual contribution.
- Provide a visible way to report inaccurate summaries, attribution errors, duplicate projects, or removal requests.
- Record corrections so subsequent processing does not silently reintroduce errors.
- Detect deleted or changed source comments during refresh and remove or revise affected public excerpts, summaries, and search entries as appropriate.
- Support suppressing removed material from future publication, including subsequent collection or processing runs.
- Review removal implications for retained source records and dataset history, not only visible website pages.

## Attribution and content rights

Public availability does not by itself establish permission to republish content. Attribution and an aggregation notice are product requirements, not substitutes for determining appropriate publication permissions and practices before launch.

Favor concise factual summaries, limited supporting excerpts where appropriate, and prominent source links. Do not imply endorsement by Hacker News or the people whose work is listed.

Proposed website notice, subject to review before publication:

> This is an independent directory of projects shared on Hacker News. Original posts, comments, and project materials belong to their respective rights holders. Summaries and categories are generated or edited for discovery and may contain errors. Each entry links to its source. Inclusion does not imply endorsement or a current invitation to collaborate. Please contact us to request a correction or removal.

The website must provide an actual correction and removal contact route before this notice is published.

## Initial release and validation

Start with three monthly threads to validate extraction quality, project grouping, useful categories, and the discovery experience. Review representative examples, including comments with multiple projects, repeated mentions, unnamed projects, explicit invitations, and relevant replies.

Expand to approximately two years of discussions once the pilot demonstrates:

- Project descriptions and extracted facts are supported by their sources.
- Visitors can find projects by problem and audience without knowing implementation details.
- Every published mention has attribution, a sharing date, and a source link.
- Project excerpts are distinguishable from generated descriptions and retain their original source attribution.
- Repeated mentions can be explored without losing their individual histories.
- Corrections and removals propagate through public pages and discovery results.

Evaluate success through discovery usefulness and information accuracy: whether visitors find relevant projects, follow through to original sources, and encounter reliable summaries. Social engagement mechanics are not a success criterion.
