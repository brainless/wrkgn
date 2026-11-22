//! Shared by `build.rs` (via `#[path]`) and unit tests in `src/lib.rs`.
//! Pure helpers for validating asset manifest entries.

/// Manifest keys become `<KEY>_URL` constants, so they must be `[a-z][a-z0-9_]*`.
pub fn validate_key(key: &str) -> Result<(), String> {
    let mut chars = key.chars();
    let ok = matches!(chars.next(), Some('a'..='z'))
        && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_'));
    if ok {
        Ok(())
    } else {
        Err(format!(
            "invalid asset manifest key {key:?}: must match [a-z][a-z0-9_]*"
        ))
    }
}

/// Fixed extension to MIME table; unknown extensions are build errors.
pub fn mime_for(filename: &str) -> Result<&'static str, String> {
    let ext = filename.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    match ext {
        "js" => Ok("text/javascript; charset=utf-8"),
        "css" => Ok("text/css; charset=utf-8"),
        "svg" => Ok("image/svg+xml"),
        "png" => Ok("image/png"),
        "woff2" => Ok("font/woff2"),
        _ => Err(format!(
            "unsupported asset extension {ext:?} in {filename:?}: add it to asset_manifest.rs"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        for key in ["script", "og_image", "font2", "a"] {
            assert!(validate_key(key).is_ok(), "{key}");
        }
        for key in [
            "", "Script", "2font", "_x", "og-image", "og image", "ö", "a/b",
        ] {
            assert!(validate_key(key).is_err(), "{key}");
        }
    }

    #[test]
    fn mime_table() {
        for (name, mime) in [
            ("a-1.js", "text/javascript; charset=utf-8"),
            ("a-1.css", "text/css; charset=utf-8"),
            ("icon-1.svg", "image/svg+xml"),
            ("og-1.png", "image/png"),
            ("f-1.woff2", "font/woff2"),
        ] {
            assert_eq!(mime_for(name).unwrap(), mime);
        }
        for name in ["a.gif", "a", "a.PNG", "a.map"] {
            assert!(mime_for(name).is_err(), "{name}");
        }
    }
}
