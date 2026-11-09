# Project discovery from Hacker News

## Overview

Create a public directory of projects and products shared in Hacker News’s recurring “Ask HN: What are you working on?” discussions. Begin with approximately the past two years of threads and continue adding new discussions.

The directory should inspire people to build and share their own work, help visitors discover interesting projects, and make it easier to find builders working on related problems. It is a discovery resource, not a social network.

## Product principles

- Focus on projects and products rather than comments as standalone content.
- Organize discovery around the problems projects tackle and the people they serve. Technology choices are not the primary discovery lens.
- Extract facts faithfully and distinguish them from editorial or automated classifications.
- Never infer that a builder wants feedback, contributors, or collaborators. Surface these requests only when explicitly stated.
- Preserve attribution, source links, and useful discussion context.
- Show when information was shared so historical mentions do not imply current availability, activity, or intent.

## Audience and needs

- People seeking inspiration want to explore what others are building and the problems they have chosen to solve.
- People with a particular need want to discover projects that may address it.
- Builders want to find related work and people with shared interests.
- Potential contributors or collaborators want to find explicit invitations and follow the source to learn more or respond.

## Scope

Collect the target discussions and their replies, then identify project-bearing contributions. Publish project entries with relevant supporting context, rather than reproducing every comment as a general discussion archive.

Replies qualify as supporting context when they add useful information about a project, such as demos, explanations, clarifications, or substantive feedback. Unrelated conversation should not appear in the public directory.

The initial product does not include accounts, private messaging, social feeds, follower relationships, or a separate discussion system. Visitors follow source or project links to engage with builders.

## Projects and mentions

A **project mention** is a specific instance of someone sharing or discussing a project in a source thread. A **project** may have multiple mentions across different threads and dates. One comment may describe multiple projects.

Keep each mention’s source, author, date, and evidence intact even when grouping it under a project. Group mentions only when there is adequate evidence that they describe the same project; leave uncertain matches separate for review.

Do not assume that everyone discussing a project is its creator. Attribute statements to their authors and identify a builder or owner only when supported by the source.

## Information to capture

For each project mention, capture where available:

- Project or product name and explicitly provided URLs.
- A concise, factual description of what it does.
- The problem it addresses and its intended audience, when supported by the source.
- Explicit requests for feedback, contributors, or collaborators, with the supporting excerpt.
- Relevant demos, explanations, and useful feedback from the discussion.
- Source thread, comment, author, and sharing date.

Missing information should remain unknown. Do not invent names, audiences, URLs, project status, or invitations. Summaries must remain grounded in source material, and quotations must be distinguishable from summaries.

Discovery categories and tags may be assigned during processing, but must be treated as classifications rather than statements made by an author. Favor understandable labels about problems, use cases, and audiences.

## Visitor experience

Visitors should be able to:

- Browse project cards that explain what a project does, whom it serves when known, and when it was shared.
- Search descriptions, problems, audiences, project names, and builder usernames.
- Narrow results using categories, tags, sharing dates, and explicit requests.
- Open a project page to see its mentions over time and relevant discussion context.
- Follow clearly attributed links to the original discussion, demos, and project websites.
- Discover other projects shared by the same builder where that relationship is supported.

Historical invitations must retain their dates and must not be presented as verified current opportunities. Recently collected information must not be confused with recently shared information.

## Separation of responsibilities

### Data collection

Identify relevant threads and collect source comments, reply relationships, author attribution, dates, and links. Preserve raw source material separately from processed records. Track collection and refresh history so source changes can be detected.

Collection must support revisiting older threads and refreshing changed or deleted comments, as well as adding new discussions.

### Parsing and processing

Use agents to identify projects, extract supported facts and explicit requests, select useful context, and assign discovery labels. Retain the evidence behind extracted information and the history of processing decisions.

Support review of ambiguous extractions, possible duplicate projects, and corrections. Processing should be repeatable without losing source provenance or overwriting reviewed corrections. Raw and processed records form a maintained, versioned source dataset.

### Static website

Publish a static discovery experience from approved processed data. Browsing, search, categories, and tags operate on the published dataset in the visitor’s browser.

The website presents project information and attribution; it does not perform source collection or agent processing. Publishing updates must propagate corrections, removals, and refreshed source information to pages and discovery data.

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
- Explicit requests are distinguishable from general sharing and retain their original context.
- Repeated mentions can be explored without losing their individual histories.
- Corrections and removals propagate through public pages and discovery results.

Evaluate success through discovery usefulness and information accuracy: whether visitors find relevant projects, follow through to original sources, and encounter reliable summaries. Social engagement mechanics are not a success criterion.
