//! Plain-text review cards for pending mentions, and the ID ranges used to approve them.
use crate::publication::PendingMention;

const LABEL: usize = 12;
const SOURCE_EXCERPT_CHARS: usize = 600;

/// Renders one bordered card per mention, followed by the pending ID ranges.
pub fn render(mentions: &[PendingMention], width: usize) -> String {
    if mentions.is_empty() {
        return "No pending mentions.\n".into();
    }
    let width = width.clamp(60, 160);
    let mut out = String::new();
    for m in mentions {
        card(&mut out, m, width);
    }
    let ids: Vec<i64> = mentions.iter().map(|m| m.id).collect();
    let ranges = format_ranges(&ids);
    out.push_str(&format!(
        "{} pending mention(s). Pending IDs: {ranges}\nApprove with: wrkgn review approve {ranges}\n",
        mentions.len()
    ));
    out
}

fn card(out: &mut String, m: &PendingMention, width: usize) {
    let inner = width - 4;
    let title = format!(" #{}  {} ", m.id, m.name.as_deref().unwrap_or("(unnamed)"));
    let meta = format!(" project {} · thread {} ", m.project_id, m.thread_id);
    let fill = (width - 3).saturating_sub(chars(&title) + chars(&meta));
    out.push_str(&format!("┌─{title}{}{meta}┐\n", "─".repeat(fill)));
    let mut source = format!(
        "{} · {} · {}",
        m.author
            .as_deref()
            .map_or("unknown".into(), |a| format!("@{a}")),
        m.shared_at.map_or("undated".into(), date),
        source_url(m.source_item_id)
    );
    if m.builder_username.is_none() {
        source.push_str(" · builder not attributed");
    }
    row(out, inner, "Source", &source);
    row(out, inner, "Description", &m.description);
    if let Some(problem) = &m.problem {
        row(out, inner, "Problem", problem);
    }
    if let Some(audience) = &m.audience {
        row(out, inner, "Audience", audience);
    }
    row(
        out,
        inner,
        "Categories",
        &list(
            &m.categories
                .iter()
                .map(|value| {
                    crate::category::Category::from_slug(value)
                        .map(|c| c.label().to_string())
                        .unwrap_or_else(|| value.clone())
                })
                .collect::<Vec<_>>(),
        ),
    );
    row(out, inner, "Audiences", &list(&m.audience_labels));
    row(out, inner, "Co-builders", &list(&m.builders));
    if let Some(excerpt) = &m.source_excerpt {
        row(out, inner, "Evidence", excerpt);
    }
    row(out, inner, "Links", &list(&m.links));
    if let Some(text) = m.source_text.as_deref().map(html_to_text) {
        out.push_str(&format!("├{}┤\n", "─".repeat(width - 2)));
        let mut first = true;
        for paragraph in excerpt(&text, SOURCE_EXCERPT_CHARS).split('\n') {
            row(out, inner, if first { "Comment" } else { "" }, paragraph);
            first = false;
        }
    }
    out.push_str(&format!("└{}┘\n\n", "─".repeat(width - 2)));
}

fn row(out: &mut String, inner: usize, label: &str, value: &str) {
    let value_width = inner - LABEL - 1;
    for (i, line) in wrap(value, value_width).iter().enumerate() {
        let label = if i == 0 { label } else { "" };
        out.push_str(&format!(
            "│ {label:<LABEL$} {line}{} │\n",
            " ".repeat(value_width - chars(line))
        ));
    }
}

fn list(values: &[String]) -> String {
    if values.is_empty() {
        "—".into()
    } else {
        values.join(", ")
    }
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

/// Greedy word wrap; words longer than the width (usually URLs) are split.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let mut word: Vec<char> = word.chars().collect();
        loop {
            let used = chars(&line);
            let gap = usize::from(used > 0);
            if used + gap + word.len() <= width {
                if gap == 1 {
                    line.push(' ');
                }
                line.extend(word.iter());
                break;
            }
            if used > 0 {
                lines.push(std::mem::take(&mut line));
                continue;
            }
            let rest = word.split_off(width);
            lines.push(word.into_iter().collect());
            word = rest;
        }
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

fn excerpt(text: &str, max: usize) -> String {
    if chars(text) <= max {
        text.to_string()
    } else {
        let cut: String = text.chars().take(max).collect();
        format!("{}…", cut.trim_end())
    }
}

/// Converts HN comment HTML to plain text with one paragraph per line.
fn html_to_text(html: &str) -> String {
    crate::html::plain_text(html)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn source_url(id: i64) -> String {
    format!("https://news.ycombinator.com/item?id={id}")
}

/// Formats a Unix timestamp as a UTC calendar date.
fn date(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Collapses sorted IDs into ranges such as `12-30 34 40-41`.
pub fn format_ranges(ids: &[i64]) -> String {
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    let mut parts = Vec::new();
    let mut i = 0;
    while i < ids.len() {
        let start = ids[i];
        while i + 1 < ids.len() && ids[i + 1] == ids[i] + 1 {
            i += 1;
        }
        parts.push(if ids[i] == start {
            start.to_string()
        } else {
            format!("{start}-{}", ids[i])
        });
        i += 1;
    }
    parts.join(" ")
}

/// Parses approval arguments: single IDs (`12`) or inclusive ranges (`12-30`).
pub fn parse_ranges(args: &[&str]) -> Result<Vec<(i64, i64)>, String> {
    if args.is_empty() {
        return Err("approve requires mention IDs or ranges, e.g. 12-30 34".into());
    }
    args.iter()
        .flat_map(|a| a.split(',').filter(|s| !s.is_empty()))
        .map(|part| {
            let parse = |s: &str| match s.trim().parse::<i64>() {
                Ok(id) if id > 0 => Ok(id),
                _ => Err(format!("invalid mention ID or range: {part}")),
            };
            match part.split_once('-') {
                Some((a, b)) => {
                    let (a, b) = (parse(a)?, parse(b)?);
                    if a > b {
                        Err(format!("invalid range: {part}"))
                    } else {
                        Ok((a, b))
                    }
                }
                None => parse(part).map(|id| (id, id)),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_round_trip() {
        assert_eq!(format_ranges(&[34, 12, 13, 14, 40, 41]), "12-14 34 40-41");
        assert_eq!(
            parse_ranges(&["12-14", "34,40-41"]).unwrap(),
            vec![(12, 14), (34, 34), (40, 41)]
        );
        assert!(parse_ranges(&["5-2"]).is_err());
        assert!(parse_ranges(&["abc"]).is_err());
        assert!(parse_ranges(&[]).is_err());
    }

    #[test]
    fn wraps_and_splits_long_words() {
        assert_eq!(wrap("aa bb cc", 5), vec!["aa bb", "cc"]);
        assert_eq!(wrap("abcdefgh", 3), vec!["abc", "def", "gh"]);
        assert_eq!(wrap("", 3), vec![""]);
    }

    #[test]
    fn formats_dates_and_comment_html() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(1_762_646_400), "2025-11-09");
        assert_eq!(
            html_to_text(
                "Hi &amp; <i>bye</i><p>It&#x27;s <a href=\"https:&#x2F;&#x2F;x.test&#x2F;\">here</a>"
            ),
            "Hi & bye\nIt's here (https://x.test/)"
        );
    }
}
