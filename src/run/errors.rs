//! Error normalization and grouping. Messages are reduced to a signature with
//! digits, IDs and URLs removed so repeats of one problem share a group, and a
//! redacted sample keeps the first full message.

use std::collections::BTreeMap;

/// Longest stored sample (characters), after redaction.
pub const SAMPLE_MAX_CHARS: usize = 2000;
/// Longest signature (characters).
pub const SIGNATURE_MAX_CHARS: usize = 200;

const REDACTED: &str = "<redacted>";

/// Names whose `name=value` (or `name:value`) values are secrets.
const SECRET_NAMES: [&str; 9] = [
    "api_key",
    "apikey",
    "api-key",
    "access_token",
    "token",
    "authorization",
    "password",
    "secret",
    "key",
];
/// Names that also take the next whitespace-separated token as their value
/// when written as `name:` or `name=` with nothing after it.
const SECRET_HEADERS: [&str; 5] = [
    "authorization",
    "api_key",
    "apikey",
    "x-api-key",
    "password",
];

/// Removes secrets: `sk-...` keys, `Bearer ...` credentials and
/// `api_key` / `token` / `authorization` style values. Whitespace is kept.
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut redact_next = false;
    for (is_space, segment) in segments(text) {
        if is_space {
            out.push_str(segment);
            continue;
        }
        let lower = segment.to_ascii_lowercase();
        let bare = lower.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        if redact_next {
            // `Authorization: Bearer xyz`: the scheme is followed by the secret.
            if matches!(bare, "bearer" | "basic") {
                out.push_str(segment);
            } else {
                out.push_str(REDACTED);
                redact_next = false;
            }
            continue;
        }
        if matches!(bare, "bearer" | "basic") {
            redact_next = true;
            out.push_str(segment);
            continue;
        }
        let (redacted, pending) = redact_token(segment, &lower);
        redact_next = pending;
        out.push_str(&redacted);
    }
    out
}

/// Alternating whitespace and non-whitespace runs.
fn segments(text: &str) -> Vec<(bool, &str)> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut current: Option<bool> = None;
    for (i, c) in text.char_indices() {
        let space = c.is_whitespace();
        match current {
            Some(kind) if kind == space => {}
            Some(kind) => {
                out.push((kind, &text[start..i]));
                start = i;
                current = Some(space);
            }
            None => current = Some(space),
        }
    }
    if let Some(kind) = current {
        out.push((kind, &text[start..]));
    }
    out
}

/// Redacts secrets inside one whitespace-free token. The flag asks the caller
/// to redact the next token too.
fn redact_token(token: &str, lower: &str) -> (String, bool) {
    let mut token = token.to_owned();
    let mut lower = lower.to_owned();
    // `sk-` API keys, possibly wrapped in punctuation.
    if let Some(at) = lower.find("sk-")
        && (at == 0 || !lower.as_bytes()[at - 1].is_ascii_alphanumeric())
    {
        let tail = &lower[at + 3..];
        let len = tail
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            .unwrap_or(tail.len());
        if len >= 5 {
            token.replace_range(at..at + 3 + len, REDACTED);
            lower = token.to_ascii_lowercase();
        }
    }
    for name in SECRET_NAMES {
        let mut from = 0;
        while let Some(found) = lower[from..].find(name) {
            let at = from + found;
            let end = at + name.len();
            from = end;
            if at > 0 && lower.as_bytes()[at - 1].is_ascii_alphanumeric() {
                continue;
            }
            // JSON style: `"name":"value"`.
            let after_name = if lower[end..].starts_with(['"', '\'']) {
                end + 1
            } else {
                end
            };
            let Some(sep) = lower[after_name..].chars().next() else {
                continue;
            };
            if sep != '=' && sep != ':' {
                continue;
            }
            let mut start = after_name + 1;
            if lower[start..].starts_with(['"', '\'']) {
                start += 1;
            }
            let rest = &lower[start..];
            let stop = rest
                .find(['&', '"', '\'', ')', ',', ';'])
                .unwrap_or(rest.len());
            // `token: ` style prose with nothing after it is not a value.
            if stop == 0 || rest.starts_with(REDACTED) || (sep == ':' && rest.starts_with('/')) {
                continue;
            }
            token.replace_range(start..start + stop, REDACTED);
            lower = token.to_ascii_lowercase();
            from = start + REDACTED.len();
        }
    }
    let trimmed = lower.trim_end_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_');
    let pending = lower.ends_with([':', '='])
        && SECRET_HEADERS
            .iter()
            .any(|name| trimmed == *name || trimmed.ends_with(&format!(" {name}")));
    (token, pending)
}

/// Collapses a message to a stable signature: secrets, URLs, long hex/UUID
/// tokens and digits are replaced by placeholders.
pub fn normalize(message: &str) -> String {
    let collapsed = collapse_whitespace(&redact(message));
    let mut out = Vec::new();
    for token in collapsed.split(' ') {
        out.push(replace_ids_and_digits(&replace_url(token)));
    }
    let signature = collapse_whitespace(&out.join(" "));
    signature.chars().take(SIGNATURE_MAX_CHARS).collect()
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Replaces `http(s)://...` inside a token, leaving trailing punctuation.
fn replace_url(token: &str) -> String {
    let lower = token.to_ascii_lowercase();
    let Some(at) = lower.find("http://").or_else(|| lower.find("https://")) else {
        return token.to_owned();
    };
    let tail = &token[at..];
    let end = tail
        .trim_end_matches([')', ']', '}', '>', ',', '.', ';', ':', '\'', '"'])
        .len();
    format!("{}<url>{}", &token[..at], &tail[end..])
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-'
}

/// Long hex/UUID-like words containing a digit become `<id>`; other digit
/// runs become `#`.
fn replace_ids_and_digits(token: &str) -> String {
    let mut out = String::with_capacity(token.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if word.is_empty() {
            return;
        }
        let hexish = word.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
        if word.len() >= 8 && hexish && word.chars().any(|c| c.is_ascii_digit()) {
            out.push_str("<id>");
        } else {
            let mut in_digits = false;
            for c in word.chars() {
                if c.is_ascii_digit() {
                    if !in_digits {
                        out.push('#');
                    }
                    in_digits = true;
                } else {
                    in_digits = false;
                    out.push(c);
                }
            }
        }
        word.clear();
    };
    for c in token.chars() {
        if is_word_char(c) {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorGroup {
    pub signature: String,
    pub stage: String,
    pub kind: String,
    pub count: usize,
    pub first_item: Option<i64>,
    pub last_item: Option<i64>,
    /// First message seen, redacted and capped at `SAMPLE_MAX_CHARS`.
    pub sample: String,
}

#[derive(Debug, Default)]
pub struct ErrorLog {
    groups: BTreeMap<(String, String, String), ErrorGroup>,
}

impl ErrorLog {
    pub fn record(&mut self, stage: &str, kind: &str, item: Option<i64>, message: &str) {
        let signature = normalize(message);
        let key = (signature.clone(), stage.to_owned(), kind.to_owned());
        let group = self.groups.entry(key).or_insert_with(|| ErrorGroup {
            signature,
            stage: stage.to_owned(),
            kind: kind.to_owned(),
            count: 0,
            first_item: None,
            last_item: None,
            sample: redact(message).chars().take(SAMPLE_MAX_CHARS).collect(),
        });
        group.count += 1;
        if group.first_item.is_none() {
            group.first_item = item;
        }
        if item.is_some() {
            group.last_item = item;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    /// Total recorded errors across groups.
    pub fn total(&self) -> usize {
        self.groups.values().map(|group| group.count).sum()
    }

    /// Count descending, then signature, stage and kind.
    pub fn groups_sorted(&self) -> Vec<&ErrorGroup> {
        let mut groups: Vec<_> = self.groups.values().collect();
        groups.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then_with(|| a.signature.cmp(&b.signature))
                .then_with(|| a.stage.cmp(&b.stage))
                .then_with(|| a.kind.cmp(&b.kind))
        });
        groups
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_the_documented_examples() {
        for (input, want) in [
            (
                "HTTP status 429 for https://hn.algolia.com/api/v1/search?page=3",
                "HTTP status # for <url>",
            ),
            (
                "error sending request for url (https://hacker-news.firebaseio.com/v0/item/4242.json)",
                "error sending request for url (<url>)",
            ),
            (
                "thread 8863 not found in run 12",
                "thread # not found in run #",
            ),
            (
                "connection reset by peer (os error 54)",
                "connection reset by peer (os error #)",
            ),
        ] {
            assert_eq!(normalize(input), want);
        }
    }

    #[test]
    fn replaces_ids_and_collapses_whitespace() {
        assert_eq!(
            normalize("  request 550e8400-e29b-41d4-a716-446655440000\n failed  "),
            "request <id> failed"
        );
        assert_eq!(normalize("deadbeef is a word"), "deadbeef is a word");
        assert_eq!(normalize("sha deadbeef01 done"), "sha <id> done");
    }

    #[test]
    fn signature_is_capped() {
        assert_eq!(normalize(&"word ".repeat(100)).chars().count(), 200);
    }

    #[test]
    fn redacts_secrets() {
        for input in [
            "auth failed with sk-ant-abc123DEF456ghi",
            "Authorization: Bearer abcdef123456",
            "bad request ?api_key=SECRETVALUE&x=1",
            "provider said token=SECRETVALUE",
            "headers {\"authorization\":\"SECRETVALUE\"}",
        ] {
            let redacted = redact(input);
            assert!(!redacted.contains("SECRETVALUE"), "{redacted}");
            assert!(!redacted.contains("abc123DEF456"), "{redacted}");
            assert!(!redacted.contains("abcdef123456"), "{redacted}");
            assert!(redacted.contains(REDACTED), "{redacted}");
            assert!(!normalize(input).contains("SECRET"));
        }
        assert_eq!(redact("keep  this\nas is"), "keep  this\nas is");
    }

    #[test]
    fn groups_by_signature_stage_and_kind() {
        let mut log = ErrorLog::default();
        log.record("collect", "transport", Some(5), "timeout fetching item 5");
        log.record("collect", "transport", Some(9), "timeout fetching item 9");
        log.record("collect", "transport", None, "timeout fetching item 12");
        log.record("extract", "transport", Some(1), "timeout fetching item 1");
        let groups = log.groups_sorted();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].count, 3);
        assert_eq!(
            (groups[0].first_item, groups[0].last_item),
            (Some(5), Some(9))
        );
        assert_eq!(groups[0].sample, "timeout fetching item 5");
        assert_eq!(log.total(), 4);
    }
}
