# Audience taxonomy

`src/audience.rs` defines the fixed audience slugs. Select zero to five distinct labels only when the source explicitly mentions the intended audience. Keep the optional factual `audience` description separately. Do not infer an audience from a category, technology, platform, or project format. Unknown audiences remain unlabeled; `general-public` is not a fallback.

| Slug | Scope |
| --- | --- |
| `developers` | Software developers and testers. |
| `system-administrators` | Infrastructure operators and self-hosters. |
| `students-learners` | People studying or learning skills. |
| `educators` | Teachers and educational practitioners. |
| `researchers` | Scientific and technical investigators. |
| `creative-professionals` | Writers, artists, designers and media creators. |
| `gamers` | People playing games. |
| `makers-hobbyists` | Hands-on makers and people pursuing hobbies. |
| `business-professionals` | People doing business, finance or operational work. |
| `founders-business-owners` | Entrepreneurs, founders and business owners. |
| `community-organizers` | People coordinating communities, events and civic work. |
| `personal-use` | Explicit individual or personal use. |
| `general-public` | Explicit broad public or consumer audience. |

Existing free-text audience labels are not automatically mapped. Reprocess source groups with the new extraction schema and review the resulting mentions before publication. The database migration preserves legacy data and approvals; the current exporter rejects unknown legacy audience labels with an actionable error. Previously staged snapshots remain usable through the website’s legacy reader.
