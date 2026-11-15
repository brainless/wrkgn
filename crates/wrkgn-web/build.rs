use std::{env, fs, path::PathBuf};
fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let dist = root.join("assets/dist");
    println!("cargo:rerun-if-changed=assets/dist");
    println!("cargo:rerun-if-env-changed=WRKGN_BUILD_ID");
    let manifest = fs::read_to_string(dist.join("manifest.json")).expect("wrkgn-web assets missing: run npm ci --prefix crates/wrkgn-web/assets && npm run build --prefix crates/wrkgn-web/assets before Cargo");
    let manifest: serde_json::Value =
        serde_json::from_str(&manifest).expect("invalid asset manifest; rebuild wrkgn-web assets");
    let mut source = String::new();
    source.push_str("pub fn embedded_asset(path: &str) -> Option<(&'static str, &'static [u8])> { match path {\n");
    for (key, mime) in [
        ("script", "text/javascript; charset=utf-8"),
        ("style", "text/css; charset=utf-8"),
        ("icon", "image/svg+xml"),
    ] {
        let name = manifest[key]
            .as_str()
            .expect("missing asset manifest entry");
        assert!(
            !name.contains('/') && !name.contains('\\') && !name.contains(".."),
            "unsafe asset filename"
        );
        let file = dist.join(name);
        assert!(
            file.is_file(),
            "built asset missing: rebuild wrkgn-web assets"
        );
        source.push_str(&format!(
            "{:?} => Some(({mime:?}, include_bytes!({:?}))),\n",
            format!("/assets/{name}"),
            file
        ));
    }
    source.push_str("_ => None } }\n");
    for key in ["script", "style", "icon"] {
        source.push_str(&format!(
            "pub const {}_URL: &str = {:?};\n",
            key.to_uppercase(),
            format!("/assets/{}", manifest[key].as_str().unwrap())
        ));
    }
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("assets.rs"),
        source,
    )
    .unwrap();
}
