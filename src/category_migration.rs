//! Explicit, offline migration of pilot free-text labels. Unknown labels abort.
use crate::{category::Category, store::Store};
use rusqlite::params;
use std::collections::{BTreeMap, BTreeSet};

pub fn legacy(label: &str) -> Result<Option<Category>, String> {
    let key = label.trim().to_ascii_lowercase().replace('_', " ");
    let group: Option<&str> = match key.as_str() {
        "ai"
        | "ai agents"
        | "ai coding agents"
        | "ai tools"
        | "artificial intelligence"
        | "chatbots"
        | "llm"
        | "machine learning"
        | "automation"
        | "llm evaluation" => Some("ai-automation"),
        "developer tools"
        | "developer-tools"
        | "dev tools"
        | "code generation"
        | "automated-testing"
        | "dsls"
        | "ide"
        | "interpreters"
        | "performance engineering"
        | "programming"
        | "programming languages"
        | "release engineering"
        | "terminal-emulators"
        | "web development" => Some("developer-tools"),
        "productivity"
        | "productivity software"
        | "productivity tools"
        | "document management"
        | "file-organization"
        | "journaling"
        | "personal knowledge management"
        | "project management"
        | "tracking" => Some("productivity"),
        "education"
        | "language-learning"
        | "original-language study tools"
        | "spaced-repetition"
        | "bible software" => Some("education"),
        "games" | "game development" | "game-development" | "gaming-tools" | "multiplayer"
        | "open-world" | "retro gaming" | "rpg" | "rts" | "tabletop rpgs" | "modding" => {
            Some("games")
        }
        "audio"
        | "books"
        | "content creation"
        | "design"
        | "fantasy"
        | "graphics software"
        | "image editing"
        | "image management"
        | "news aggregation"
        | "novel"
        | "photo organization"
        | "procedural generation"
        | "science fiction"
        | "storytelling"
        | "technical writing"
        | "text-analysis"
        | "typography"
        | "video content"
        | "writing" => Some("creative-media"),
        "data pipeline"
        | "data rooms"
        | "data visualization"
        | "databases"
        | "knowledge-graph"
        | "maps"
        | "relational-databases"
        | "search"
        | "storage-engines" => Some("data-analytics"),
        "accounting"
        | "b2b saas"
        | "event ticketing"
        | "finance"
        | "fintech"
        | "fundraising"
        | "licensing"
        | "marketplace"
        | "professional services"
        | "proptech"
        | "repair services"
        | "small business"
        | "tax"
        | "trading" => Some("business-finance"),
        "civic tech" | "communications" | "community" | "politics" | "social"
        | "voting systems" => Some("community-civic"),
        "analog electronics"
        | "asic"
        | "cnc"
        | "diy"
        | "diy vehicle build"
        | "e-ink"
        | "electric vehicles"
        | "electronics"
        | "electronics-repair"
        | "hardware"
        | "machining"
        | "measurement instrumentation"
        | "open-source hardware"
        | "radio"
        | "repair"
        | "retro computing" => Some("hardware-diy"),
        "airflow"
        | "ansible"
        | "backup"
        | "data storage"
        | "devops"
        | "distributed storage"
        | "dns"
        | "home server software"
        | "homelab"
        | "infrastructure"
        | "infrastructure-as-code"
        | "internet technology"
        | "linux-distribution"
        | "media-server"
        | "networking"
        | "operating systems"
        | "paas"
        | "self-hosted"
        | "self-hosted apps"
        | "self-hosting"
        | "terraform"
        | "web hosting"
        | "ai-llm-infrastructure" => Some("infrastructure-hosting"),
        "authentication/security" | "data-ownership" | "encryption" | "privacy" => {
            Some("security-privacy")
        }
        "mathematics"
        | "research"
        | "research replication"
        | "science"
        | "signal processing"
        | "simulation"
        | "space weather" => Some("science-research"),
        "fitness" => Some("health-fitness"),
        "calligraphy"
        | "cooking"
        | "food"
        | "fountain-pens"
        | "gardening"
        | "genealogy"
        | "handwriting"
        | "home-improvement"
        | "reading-recommendations"
        | "thanksgiving"
        | "transportation"
        | "worldbuilding" => Some("lifestyle-hobbies"),
        "ai integration"
        | "ai-assisted"
        | "ai-assisted development"
        | "ai-assisted-development"
        | "ai/mcp integration"
        | "browser extensions"
        | "browsers"
        | "calculators"
        | "command-line-tool"
        | "desktop app"
        | "experiment"
        | "experiments"
        | "graphical-user-interface"
        | "gui applications"
        | "go"
        | "hackathon projects"
        | "linux"
        | "local-first software"
        | "local-only setup"
        | "macos"
        | "macos apps"
        | "mobile app"
        | "mobile apps"
        | "open source"
        | "open-source"
        | "personal apps"
        | "personal software"
        | "personal tools"
        | "piracy"
        | "project idea"
        | "saas"
        | "side project"
        | "software"
        | "tools"
        | "unraid app"
        | "web platform" => None,
        _ => return Err(format!("unknown legacy category: {label}")),
    };
    Ok(group.and_then(Category::from_slug))
}

pub fn normalize(store: &mut Store) -> Result<(usize, usize), String> {
    let tx = store
        .connection()
        .transaction()
        .map_err(|e| e.to_string())?;
    let rows: Vec<(i64, String)> = {
        let mut st = tx.prepare("SELECT mention_id,value FROM mention_labels WHERE kind='category' ORDER BY mention_id,value").map_err(|e|e.to_string())?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<_>>()
            .map_err(|e| e.to_string())?
    };
    let mut replacement: BTreeMap<i64, BTreeSet<&'static str>> = BTreeMap::new();
    let mut dropped = 0;
    let mut unknown = BTreeSet::new();
    for (id, label) in &rows {
        let category = Category::from_slug(label)
            .map(Some)
            .ok_or_else(|| label.clone())
            .or_else(|_| legacy(label));
        match category {
            Ok(Some(c)) => {
                replacement.entry(*id).or_default().insert(c.slug());
            }
            Ok(None) => dropped += 1,
            Err(_) => {
                unknown.insert(label.clone());
            }
        }
    }
    if !unknown.is_empty() {
        return Err(format!(
            "unknown categories; no changes made: {}",
            unknown.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    if let Some((id, labels)) = replacement.iter().find(|(_, v)| v.len() > 5) {
        return Err(format!(
            "mention {id} would have {} categories; no changes made",
            labels.len()
        ));
    }
    tx.execute("DELETE FROM mention_labels WHERE kind='category'", [])
        .map_err(|e| e.to_string())?;
    for (id, labels) in &replacement {
        for label in labels {
            tx.execute(
                "INSERT INTO mention_labels(mention_id,kind,value) VALUES(?1,'category',?2)",
                params![id, label],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok((rows.len(), dropped))
}
