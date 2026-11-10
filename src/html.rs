//! Converts HN's HTML-escaped comment bodies to the plain text that the model
//! reads and that its output is validated against.

/// Decodes entities, turns `<p>` into a paragraph break, replaces links with
/// their full target URL, and drops other tags while keeping their content.
pub fn plain_text(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        out.push_str(&decode_entities(&rest[..start]));
        let after = &rest[start + 1..];
        let Some(end) = after.find('>') else {
            // Not a tag; HN escapes literal angle brackets.
            out.push('<');
            rest = after;
            continue;
        };
        let tag = &after[..end];
        rest = &after[end + 1..];
        let name = tag
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        match name.as_str() {
            "p" if !tag.starts_with('/') => out.push_str("\n\n"),
            "a" if !tag.starts_with('/') => {
                let (inner, remainder) = rest.split_once("</a>").unwrap_or((rest, ""));
                rest = remainder;
                let label = plain_text(inner);
                match attribute(tag, "href") {
                    Some(href) => out.push_str(&link(&label, &decode_entities(&href))),
                    None => out.push_str(&label),
                }
            }
            _ => {}
        }
    }
    out.push_str(&decode_entities(rest));
    out.trim().to_string()
}

/// The full URL when the label is that URL or a shortened form of it (HN cuts
/// long link text with "..."); otherwise the label followed by the URL.
fn link(label: &str, href: &str) -> String {
    let label = label.trim();
    let prefix = label.trim_end_matches("...").trim_end_matches('…');
    let bare = |s: &str| {
        s.trim_start_matches("https://")
            .trim_start_matches("http://")
            .to_string()
    };
    if label.is_empty() || bare(href).starts_with(&bare(prefix)) {
        href.to_string()
    } else {
        format!("{label} ({href})")
    }
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let at = lower.find(&format!("{name}="))? + name.len() + 1;
    let value = &tag[at..];
    let quote = value.chars().next().filter(|c| matches!(c, '"' | '\''));
    match quote {
        Some(q) => value[1..].split(q).next().map(str::to_string),
        None => value.split(char::is_whitespace).next().map(str::to_string),
    }
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let decoded = rest
            .find(';')
            .filter(|end| *end <= 10)
            .and_then(|end| entity(&rest[1..end]).map(|c| (c, end)));
        match decoded {
            Some((c, end)) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        _ => {
            let number = name.strip_prefix('#')?;
            let code = match number.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => number.parse().ok()?,
            };
            char::from_u32(code)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::plain_text;

    #[test]
    fn decodes_entities_and_paragraphs() {
        assert_eq!(
            plain_text(
                "If that&#x27;s not right:<p><i>&quot;Yes &amp; 1&#x2F;3 &#8212; &lt;ok&gt;&quot;</i>"
            ),
            "If that's not right:\n\n\"Yes & 1/3 — <ok>\""
        );
        assert_eq!(plain_text("AT&T &unknown; &#xZZ;"), "AT&T &unknown; &#xZZ;");
    }

    #[test]
    fn link_text_equal_to_target_becomes_the_url() {
        assert_eq!(
            plain_text(
                "<a href=\"https:&#x2F;&#x2F;pahunchi.netlify.app&#x2F;\" rel=\"nofollow\">https:&#x2F;&#x2F;pahunchi.netlify.app&#x2F;</a><p>A free app"
            ),
            "https://pahunchi.netlify.app/\n\nA free app"
        );
    }

    #[test]
    fn truncated_link_text_becomes_the_full_url() {
        assert_eq!(
            plain_text(
                "NameTheory: <a href=\"https:&#x2F;&#x2F;apps.apple.com&#x2F;us&#x2F;app&#x2F;nametheory-domain-search&#x2F;id6808794544\">https:&#x2F;&#x2F;apps.apple.com&#x2F;us&#x2F;app&#x2F;nametheory-domain-search&#x2F;id680...</a><p>It uses AI"
            ),
            "NameTheory: https://apps.apple.com/us/app/nametheory-domain-search/id6808794544\n\nIt uses AI"
        );
    }

    #[test]
    fn unrelated_link_text_keeps_both() {
        assert_eq!(
            plain_text("see <a href=\"https:&#x2F;&#x2F;example.com&#x2F;x\">my site</a>."),
            "see my site (https://example.com/x)."
        );
    }

    #[test]
    fn strips_code_markup_and_keeps_content() {
        assert_eq!(
            plain_text("Run:<p><pre><code>cargo run &amp;&amp; ls\n  -l</code></pre>"),
            "Run:\n\ncargo run && ls\n  -l"
        );
    }
}
