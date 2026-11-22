use std::{env, fs, path::PathBuf};
#[path = "asset_manifest.rs"]
mod asset_manifest;
use asset_manifest::{mime_for, validate_key};
fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let dist = root.join("assets/dist");
    println!("cargo:rerun-if-changed=assets/dist");
    println!("cargo:rerun-if-changed=asset_manifest.rs");
    println!("cargo:rerun-if-env-changed=WRKGN_BUILD_ID");
    let manifest = fs::read_to_string(dist.join("manifest.json")).expect("wrkgn-web assets missing: run npm ci --prefix crates/wrkgn-web/assets && npm run build --prefix crates/wrkgn-web/assets before Cargo");
    let manifest: serde_json::Value =
        serde_json::from_str(&manifest).expect("invalid asset manifest; rebuild wrkgn-web assets");
    let mut source = String::new();
    source.push_str("pub fn embedded_asset(path: &str) -> Option<(&'static str, &'static [u8])> { match path {\n");
    let entries = manifest
        .as_object()
        .expect("asset manifest must be a flat key to filename map");
    let mut constants = String::new();
    for (key, value) in entries {
        if let Err(message) = validate_key(key) {
            panic!("{message}");
        }
        let name = value
            .as_str()
            .expect("asset manifest values must be filenames");
        assert!(
            !name.contains('/') && !name.contains('\\') && !name.contains(".."),
            "unsafe asset filename"
        );
        let mime = mime_for(name).unwrap_or_else(|message| panic!("{message}"));
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
        constants.push_str(&format!(
            "pub const {}_URL: &str = {:?};\n",
            key.to_uppercase(),
            format!("/assets/{name}")
        ));
    }
    source.push_str("_ => None } }\n");
    source.push_str(&constants);
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("assets.rs"),
        source,
    )
    .unwrap();
}
