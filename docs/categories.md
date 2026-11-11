# Discovery category taxonomy

`src/category.rs` defines the 15 fixed category slugs, visible labels, and definitions. The extraction schema accepts zero to five distinct categories for each mention. Categories classify a project's core purpose. Building with AI, using a particular language, shipping as a desktop app, or being an early idea does not by itself qualify for a category. A project's union across multiple mentions may exceed five.

The Rust exporter writes the same taxonomy into each public snapshot manifest. Discovery records store slugs; the website reads the manifest for visible labels. The taxonomy metadata participates in the snapshot version hash, so changes to a label or definition create a new snapshot.

| Slug | Label | Scope |
| --- | --- | --- |
| `ai-automation` | AI & automation | AI, machine learning, or automation is central to the product's function. |
| `developer-tools` | Developer tools | Writing, testing, releasing, and maintaining software. |
| `productivity` | Productivity | Organizing work, notes, tasks, and personal information. |
| `education` | Education | Learning, teaching, and study. |
| `games` | Games | Playable games and gaming tools. |
| `creative-media` | Creative tools & media | Writing, design, images, audio, video, and media. |
| `data-analytics` | Data & analytics | Databases, data processing, visualization, and analysis. |
| `business-finance` | Business & finance | Commerce, operations, finance, and services. |
| `community-civic` | Community & civic | Communities, social coordination, and public affairs. |
| `hardware-diy` | Hardware & DIY | Devices, electronics, fabrication, and repair. |
| `infrastructure-hosting` | Infrastructure & hosting | Hosting, networking, storage, operating systems, and infrastructure. |
| `security-privacy` | Security & privacy | Authentication, protection, encryption, and privacy. |
| `science-research` | Science & research | Scientific inquiry, research, and technical investigation. |
| `health-fitness` | Health & fitness | Health, wellness, and exercise. |
| `lifestyle-hobbies` | Lifestyle & hobbies | Personal interests, food, home, and hobbies. |

## Pilot data normalization

Run `wrkgn categories normalize --db local-data/wrkgn.sqlite` explicitly before exporting a database with free-text category labels. The finite mappings live in `src/category_migration.rs`; an unknown label aborts the whole operation before any write. It replaces category labels in one transaction, deduplicates synonymous labels per mention, and leaves approvals, corrections, IDs, source provenance, and slugs untouched. Running it again is safe.

Representative mappings: `AI agents` → `ai-automation`, `developer-tools` → `developer-tools`, `game-development` → `games`, `data visualization` → `data-analytics`, `event ticketing` → `business-finance`, and `ai-llm-infrastructure` → `infrastructure-hosting` after checking its OIDC relay description. Generic format, stage, and implementation labels are deliberately removed: `software`, `tools`, `project idea`, `experiment`, `command-line-tool`, `macOS`, `go`, and `AI-assisted development`. The exact full allowlist and drop list are auditable in the migration source.
