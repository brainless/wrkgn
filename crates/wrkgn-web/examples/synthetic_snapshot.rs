//! Deliberate public-only fixture generator; never opens the working database.
use sha2::{Digest, Sha256};
use std::{fs, path::Path};
use wrkgn_public_snapshot::{self as public, Detail, Discovery, Evidence, Mention, Metadata};

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 || args.len() > 4 {
        return Err(
            "usage: synthetic_snapshot <export-root> <new-output-root> [count=100000] [seed=0]"
                .into(),
        );
    }
    let count: usize = args
        .get(2)
        .map_or(Ok(100_000), |s| s.parse())
        .map_err(|_| "invalid count")?;
    if count > 1_000_000 {
        return Err("count must be <=1000000".into());
    }
    let source = Path::new(&args[0]);
    let output = Path::new(&args[1]);
    if output.exists() {
        return Err("output must be a new directory".into());
    }
    // Taxonomy comes from the source snapshot's validated SQLite database.
    let selection = public::select(source)?;
    let (_, source_metadata) = public::open_validated(&selection.database)?;
    let hash = Sha256::digest(format!(
        "synthetic-schema6-{count}-{}",
        args.get(3).map_or("0", String::as_str)
    ));
    let version = format!("{hash:x}")[..16].to_owned();
    let metadata = Metadata {
        schema_version: 6,
        version: version.clone(),
        exported_at: 0,
        categories: source_metadata.categories,
        audiences: source_metadata.audiences,
    };
    let mut rows = Vec::with_capacity(count);
    let mut details = Vec::with_capacity(count);
    for n in 1..=count {
        let id = n as i64;
        let name = Some(format!("Synthetic field notes {id}"));
        let description = format!(
            "Synthetic project {id} for developers collecting field notes, café observations and research. {}",
            "public fixture text ".repeat(8)
        );
        let builder = format!("samplebuilder{}", n % 100);
        let shared_at = 1_700_000_000 + id;
        let source_url = format!("https://news.ycombinator.com/item?id={id}");
        let mention = Mention {
            name: name.clone(),
            description: description.clone(),
            problem: Some("Organize observations".into()),
            audience: Some("Developers".into()),
            source_item_id: id,
            source_url: source_url.clone(),
            author: Some(builder.clone()),
            shared_at,
            source_excerpt: "Synthetic field notes".into(),
            builder_usernames: vec![builder.clone()],
            links: vec![format!("https://example.test/project-{id}")],
            evidence: vec![Evidence {
                source_item_id: id,
                source_field: "body".into(),
                value: "Synthetic field notes".into(),
                author: Some(builder.clone()),
                shared_at,
                source_url,
            }],
        };
        let slug = format!("synthetic-{id}");
        rows.push(Discovery {
            id,
            slug: slug.clone(),
            name: name.clone(),
            description: description.clone(),
            problem: mention.problem.clone(),
            audience: mention.audience.clone(),
            builder_usernames: vec![builder.clone()],
            categories: vec![
                if n % 2 == 0 {
                    "productivity"
                } else {
                    "developer-tools"
                }
                .into(),
            ],
            audience_labels: vec!["developers".into()],
            latest_shared_at: shared_at,
            comment_count: (n % 1000) as i64,
            searchable_text: format!(
                "{} {description} Organize observations Developers {builder}",
                name.as_deref().unwrap()
            ),
        });
        details.push(Detail {
            id,
            slug,
            name,
            mentions: vec![mention],
        });
    }
    rows.sort_by_key(|p| (std::cmp::Reverse(p.latest_shared_at), p.id));
    let directory = output.join("snapshots").join(&version);
    fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    public::write_database(
        &directory.join(public::DATABASE_FILE),
        &metadata,
        &rows,
        &details,
    )?;
    let write = |p: &Path, v: &serde_json::Value| {
        fs::write(p, serde_json::to_vec(v).unwrap()).map_err(|e| e.to_string())
    };
    write(
        &directory.join("manifest.json"),
        &serde_json::json!({"schema_version":6,"version":version,"database":"public.sqlite","project_count":count}),
    )?;
    write(
        &output.join("current.json"),
        &serde_json::json!({"schema_version":6,"version":version,"snapshot":format!("snapshots/{version}/manifest.json")}),
    )?;
    println!(
        "Generated {count} synthetic projects at {} version={version}",
        output.display()
    );
    Ok(())
}
