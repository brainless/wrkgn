//! Deliberate public-only fixture generator; never opens the working database.
use sha2::{Digest, Sha256};
use std::{fs, path::Path};
use wrkgn_public_snapshot::{self as public, Detail, Discovery, Evidence, Mention, Metadata};

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 || args.len() > 4 {
        return Err("usage: synthetic_snapshot <public-JSON-root> <new-output-root> [count=100000] [seed=0]".into());
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
    let read = |p: &Path| -> Result<serde_json::Value, String> {
        serde_json::from_slice(&fs::read(p).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    };
    let pointer = read(&source.join("current.json"))?;
    let relative = pointer["snapshot"]
        .as_str()
        .ok_or("missing public manifest")?;
    if Path::new(relative)
        .components()
        .any(|p| !matches!(p, std::path::Component::Normal(_)))
    {
        return Err("unsafe public manifest path".into());
    }
    let manifest = read(&source.join(relative))?;
    let hash = Sha256::digest(format!(
        "synthetic-schema6-{count}-{}",
        args.get(3).map_or("0", String::as_str)
    ));
    let version = format!("{hash:x}")[..16].to_owned();
    let metadata = Metadata {
        schema_version: 6,
        version: version.clone(),
        exported_at: 0,
        categories: serde_json::from_value(manifest["categories"].clone())
            .map_err(|e| e.to_string())?,
        audiences: serde_json::from_value(manifest["audiences"].clone())
            .map_err(|e| e.to_string())?,
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
    fs::create_dir_all(directory.join("projects")).map_err(|e| e.to_string())?;
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
        &directory.join("discovery.json"),
        &serde_json::to_value(&rows).unwrap(),
    )?;
    for detail in &details {
        let mut value = serde_json::to_value(detail).unwrap();
        for mention in value["mentions"].as_array_mut().unwrap() {
            for evidence in mention["evidence"].as_array_mut().unwrap() {
                evidence["supports"] = serde_json::json!([]);
            }
        }
        write(
            &directory
                .join("projects")
                .join(format!("{}.json", detail.slug)),
            &value,
        )?;
    }
    write(
        &directory.join("manifest.json"),
        &serde_json::json!({"schema_version":6,"version":version,"discovery":"discovery.json","project_details":"projects/<slug>.json","database":"public.sqlite","project_count":count,"categories":metadata.categories,"audiences":metadata.audiences}),
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
