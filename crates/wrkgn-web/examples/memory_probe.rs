//! Maintainer-run resident memory measurement with one/two snapshots retained.
use std::{hint::black_box, path::Path, process::Command};
use wrkgn_web::snapshot::Snapshot;
fn rss() -> Result<String, String> {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("ps failed".into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.len() > 2 {
        return Err("usage: memory_probe <snapshot-root> [next-snapshot-root]".into());
    }
    let first = Snapshot::load(Path::new(&args[0]), 4)?;
    println!(
        "one_snapshot version={} projects={} resident_kib={}",
        first.metadata.version,
        first.discovery.projects.len(),
        rss()?
    );
    let second = Snapshot::load(Path::new(args.get(1).unwrap_or(&args[0])), 4)?;
    println!(
        "two_snapshots versions={},{} resident_kib={}",
        first.metadata.version,
        second.metadata.version,
        rss()?
    );
    black_box((&first, &second));
    Ok(())
}
