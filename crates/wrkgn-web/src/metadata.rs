//! Pure helpers for titles, descriptions and structured data. No I/O; all lengths
//! count Unicode scalar values (`chars()`), never bytes.
use serde_json::{Value, json};

pub const SITE_SUFFIX: &str = " · wrkgn";
pub const TITLE_BUDGET: usize = 60;
pub const DESCRIPTION_BUDGET: usize = 155;
/// A truncated description fragment shorter than this (excluding the ellipsis)
/// is not useful, so the title falls back to the name alone.
pub const MIN_FRAGMENT: usize = 20;
const ELLIPSIS: char = '…';
const SEPARATORS: &str = "-–—:;,.|·•/";

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Shortens `text` to at most `max` chars at a word boundary, ending in an
/// ellipsis when anything was cut. Falls back to a hard cut for one long word.
pub fn truncate_words(text: &str, max: usize) -> String {
    let text = collapse(text);
    if text.chars().count() <= max {
        return text;
    }
    let keep: Vec<char> = text.chars().take(max.saturating_sub(1)).collect();
    // A cut is on a boundary when the next char is whitespace.
    let at_boundary = text
        .chars()
        .nth(keep.len())
        .is_some_and(char::is_whitespace);
    let end = if at_boundary {
        keep.len()
    } else {
        keep.iter()
            .rposition(|c| c.is_whitespace())
            .unwrap_or(keep.len())
    };
    let cut: String = keep[..end].iter().collect();
    let cut = cut.trim_end_matches(|c: char| c.is_whitespace() || SEPARATORS.contains(c));
    format!("{cut}{ELLIPSIS}")
}

/// Meta, Open Graph and Twitter description.
pub fn meta_description(text: &str) -> String {
    truncate_words(text, DESCRIPTION_BUDGET)
}

/// Removes a case-insensitive `name` prefix (ending at a word boundary) and any
/// following separators from `description`.
fn strip_name_prefix<'a>(name: &str, description: &'a str) -> &'a str {
    let mut rest = description.char_indices();
    for n in name.chars() {
        match rest.next() {
            Some((_, d)) if n.to_lowercase().eq(d.to_lowercase()) => {}
            _ => return description,
        }
    }
    let tail = rest.next().map_or("", |(i, _)| &description[i..]);
    if tail.chars().next().is_some_and(char::is_alphanumeric) {
        return description;
    }
    tail.trim_start_matches(|c: char| c.is_whitespace() || SEPARATORS.contains(c))
}

/// `<name> – <short description> · wrkgn`, within `TITLE_BUDGET` chars when a
/// useful description fragment fits, otherwise `<name> · wrkgn`. `strip` is
/// false for unnamed projects, whose fallback name is not a real prefix.
pub fn project_title(name: &str, description: &str, strip: bool) -> String {
    let rest = if strip {
        strip_name_prefix(name, description)
    } else {
        description
    };
    let rest = collapse(rest);
    let used = name.chars().count() + " – ".chars().count() + SITE_SUFFIX.chars().count();
    let budget = TITLE_BUDGET.saturating_sub(used);
    let first_word = rest.split(' ').next().unwrap_or("").chars().count();
    // The first word must fit alongside the ellipsis unless the whole rest fits.
    let fits = !rest.is_empty() && (rest.chars().count() <= budget || first_word < budget);
    if !fits {
        return format!("{name}{SITE_SUFFIX}");
    }
    let fragment = truncate_words(&rest, budget);
    if fragment.ends_with(ELLIPSIS) && fragment.chars().count() - 1 < MIN_FRAGMENT {
        return format!("{name}{SITE_SUFFIX}");
    }
    format!("{name} – {fragment}{SITE_SUFFIX}")
}

/// Serializes JSON for an inline `application/ld+json` block. `<` is escaped so
/// source-derived text can never close or reopen the script element.
pub fn json_ld(value: &Value) -> String {
    value.to_string().replace('<', "\\u003c")
}

pub fn website(origin: &str) -> String {
    json_ld(&json!({
        "@context": "https://schema.org",
        "@type": "WebSite",
        "name": "wrkgn",
        "url": format!("{origin}/"),
    }))
}

/// Minimal `CollectionPage` for landing and index pages.
pub fn collection_page(name: &str, description: &str, url: &str) -> String {
    json_ld(&json!({
        "@context": "https://schema.org",
        "@type": "CollectionPage",
        "name": name,
        "description": description,
        "url": url,
    }))
}

pub fn hn_user_url(username: &str) -> String {
    let mut url = url::Url::parse("https://news.ycombinator.com/user").expect("static url");
    url.query_pairs_mut().append_pair("id", username);
    url.into()
}

pub fn utc(seconds: i64) -> Option<String> {
    chrono::DateTime::from_timestamp(seconds, 0).map(|d| d.format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

/// `datePublished` is the earliest mention date, never `latest_shared_at`, and
/// no `dateModified` is emitted because no modification time exists.
pub fn project_json_ld(
    name: &str,
    description: &str,
    url: &str,
    builders: &[String],
    first_shared: Option<i64>,
) -> String {
    let mut value = json!({
        "@context": "https://schema.org",
        "@type": "CreativeWork",
        "name": name,
        "description": description,
        "url": url,
    });
    if let Some(date) = first_shared.and_then(utc) {
        value["datePublished"] = json!(date);
    }
    if !builders.is_empty() {
        value["author"] = builders
            .iter()
            .map(|b| json!({"@type": "Person", "name": b, "url": hn_user_url(b)}))
            .collect();
    }
    json_ld(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_at_word_boundary_with_ellipsis() {
        assert_eq!(truncate_words("one two three", 20), "one two three");
        assert_eq!(truncate_words("one two three four", 14), "one two three…");
        assert_eq!(truncate_words("one two three four", 13), "one two…");
        assert_eq!(truncate_words("  spaced\n out  ", 50), "spaced out");
        assert_eq!(truncate_words("abcdefghij", 5), "abcd…");
    }

    #[test]
    fn counts_chars_not_bytes() {
        let text = "café ".repeat(60);
        let out = meta_description(&text);
        assert!(out.chars().count() <= DESCRIPTION_BUDGET);
        assert!(out.ends_with("café…"), "{out}");
        let wide = "日本語のテキスト ".repeat(40);
        let out = meta_description(&wide);
        assert!(out.chars().count() <= DESCRIPTION_BUDGET);
        assert!(out.ends_with('…'));
        assert_eq!(truncate_words("日本語日本語", 4), "日本語…");
    }

    #[test]
    fn real_length_description_stays_in_budget() {
        let text = "A self-hosted tool that watches your build pipelines and explains failures in plain language, so small teams spend less time reading logs and more time shipping fixes to production.";
        assert!(text.chars().count() > DESCRIPTION_BUDGET);
        let out = meta_description(text);
        assert!(out.chars().count() <= DESCRIPTION_BUDGET);
        assert!(out.ends_with('…') && !out.contains("  "));
        assert!(text.starts_with(out.trim_end_matches('…')));
    }

    #[test]
    fn titles_strip_name_prefix_case_insensitively() {
        assert_eq!(
            project_title("mib", "MIB: a tiny Markdown editor", true),
            "mib – a tiny Markdown editor · wrkgn"
        );
        assert_eq!(
            project_title("Mib", "mib - tiny editor", true),
            "Mib – tiny editor · wrkgn"
        );
        // Prefix inside a longer word is not the name.
        assert_eq!(
            project_title("mib", "mibble is a game", true),
            "mib – mibble is a game · wrkgn"
        );
        // Nothing left after stripping.
        assert_eq!(project_title("mib", "MIB", true), "mib · wrkgn");
        assert_eq!(project_title("mib", "mib — ", true), "mib · wrkgn");
    }

    #[test]
    fn titles_fit_budget_and_handle_edges() {
        let d = "Extremely detailed explanation of everything this project does and why";
        let t = project_title("Tool", d, true);
        assert!(t.chars().count() <= TITLE_BUDGET, "{t}");
        assert!(t.starts_with("Tool – Extremely") && t.ends_with("… · wrkgn"));
        // Long name leaves no room: name alone, never truncated.
        let long = "A very long project name that fills the whole budget on its own";
        assert_eq!(project_title(long, "tiny", true), format!("{long} · wrkgn"));
        assert_eq!(project_title("x", "", true), "x · wrkgn");
        // Multibyte.
        let t = project_title("Café", "Ünïcödé ".repeat(20).trim(), true);
        assert!(t.chars().count() <= TITLE_BUDGET, "{t}");
    }

    #[test]
    fn short_truncated_fragment_falls_back_to_name() {
        // 38-char name leaves a 11-char budget: "A more readable…" would be too short.
        let name = "A more readable take on HN, extended!!";
        let d = "A more readable alternative web interface for Hacker News";
        assert_eq!(project_title(name, d, true), format!("{name} · wrkgn"));
        // A short description that fits whole is still used.
        assert_eq!(
            project_title(name, "tiny", true),
            format!("{name} – tiny · wrkgn")
        );
    }

    #[test]
    fn unnamed_projects_do_not_strip() {
        let t = project_title("Unnamed project", "Unnamed tracker", false);
        assert!(t.starts_with("Unnamed project – Unnamed tracker"), "{t}");
    }

    #[test]
    fn json_ld_escapes_script_close_and_encodes_author_url() {
        let out = project_json_ld(
            "n</script>",
            "d \"q\" </script><script>alert(1)",
            "https://wrkgn.com/projects/x/",
            &["a b&c".into()],
            Some(0),
        );
        assert!(!out.contains('<'), "{out}");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["name"], "n</script>");
        assert_eq!(v["datePublished"], "1970-01-01T00:00:00Z");
        assert_eq!(
            v["author"][0]["url"],
            "https://news.ycombinator.com/user?id=a+b%26c"
        );
        assert!(v.get("dateModified").is_none());
    }
}
