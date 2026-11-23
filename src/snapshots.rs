//! Explicit, offline pruning of old local export snapshots.
//!
//! `export` never removes anything. This module is only reached through
//! `wrkgn snapshots prune`. Nothing here opens the working database or the
//! network, and every deletion is a direct child of `<export>/snapshots`.
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Completed versions kept in addition to the one `current.json` selects.
pub const DEFAULT_KEEP: usize = 2;

#[derive(Debug, Default)]
pub struct PruneReport {
    pub removed_versions: Vec<(String, u64)>,
    pub removed_staging: Vec<(String, u64)>,
    pub kept: Vec<String>,
    pub skipped: Vec<String>,
}

impl PruneReport {
    pub fn reclaimed_bytes(&self) -> u64 {
        self.removed_versions
            .iter()
            .chain(&self.removed_staging)
            .map(|(_, bytes)| bytes)
            .sum()
    }
}

fn is_version(name: &str) -> bool {
    name.len() == 16 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Parses `.staging-<pid>-<version>`.
fn staging_pid(name: &str) -> Option<u32> {
    let rest = name.strip_prefix(".staging-")?;
    let (pid, version) = rest.split_once('-')?;
    if !is_version(version) {
        return None;
    }
    pid.parse().ok()
}

/// Conservative: only a clearly absent process counts as dead.
pub(crate) fn process_alive(pid: u32) -> bool {
    match Command::new("ps")
        .args(["-p", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) => status.success(),
        Err(_) => true,
    }
}

fn dir_size(path: &Path) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    if !meta.is_dir() {
        return meta.len();
    }
    fs::read_dir(path)
        .map(|entries| entries.flatten().map(|entry| dir_size(&entry.path())).sum())
        .unwrap_or(0)
}

fn current_version(export: &Path) -> Result<String, String> {
    let pointer = export.join("current.json");
    let value: serde_json::Value = serde_json::from_slice(
        &fs::read(&pointer).map_err(|e| format!("cannot read {}: {e}", pointer.display()))?,
    )
    .map_err(|e| format!("invalid {}: {e}", pointer.display()))?;
    let version = value["version"].as_str().unwrap_or_default();
    if !is_version(version)
        || value["snapshot"].as_str() != Some(&format!("snapshots/{version}/manifest.json"))
    {
        return Err(format!("unsafe or unsupported {}", pointer.display()));
    }
    Ok(version.to_string())
}

pub fn human(bytes: u64) -> String {
    let mut value = bytes as f64;
    for unit in ["B", "KiB", "MiB", "GiB"] {
        if value < 1024.0 {
            return format!("{value:.1} {unit}");
        }
        value /= 1024.0;
    }
    format!("{value:.1} TiB")
}

/// Removes old completed snapshots and abandoned staging directories.
pub fn prune(export: &Path, keep: usize, dry_run: bool) -> Result<PruneReport, String> {
    let current = current_version(export)?;
    let root = export.join("snapshots");
    let root_meta = fs::symlink_metadata(&root)
        .map_err(|e| format!("cannot inspect {}: {e}", root.display()))?;
    if !root_meta.is_dir() {
        return Err(format!("{} is not a real directory", root.display()));
    }
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    if !root.join(&current).join("manifest.json").is_file() {
        return Err(format!(
            "current.json selects version {current}, but that snapshot is missing; refusing to prune"
        ));
    }

    let mut report = PruneReport::default();
    let mut completed: Vec<(std::time::SystemTime, String)> = Vec::new();
    let mut stale_staging = Vec::new();
    for entry in fs::read_dir(&root).map_err(|e| e.to_string())?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let meta = fs::symlink_metadata(entry.path()).map_err(|e| e.to_string())?;
        if name.starts_with(".staging-") {
            match staging_pid(&name) {
                Some(pid) if process_alive(pid) => {
                    return Err(format!(
                        "{name} belongs to running process {pid}; wait for the export to finish before pruning"
                    ));
                }
                Some(_) if meta.is_dir() => stale_staging.push(name),
                _ => report.skipped.push(name),
            }
        } else if is_version(&name) && meta.is_dir() {
            completed.push((meta.modified().map_err(|e| e.to_string())?, name));
        } else {
            report.skipped.push(name);
        }
    }
    // Newest first; the name breaks timestamp ties deterministically.
    completed.sort_by(|a, b| b.cmp(a));
    let mut others = 0;
    let mut doomed = Vec::new();
    for (_, name) in completed {
        if name == current {
            report.kept.push(name);
        } else if others < keep {
            others += 1;
            report.kept.push(name);
        } else {
            doomed.push(name);
        }
    }
    stale_staging.sort();
    doomed.sort();
    for (names, staging) in [(doomed, false), (stale_staging, true)] {
        for name in names {
            let path = root.join(&name);
            // Defense in depth: a direct, non-symlink child of the canonical root.
            if path.parent() != Some(root.as_path())
                || fs::symlink_metadata(&path)
                    .map(|m| m.file_type().is_symlink() || !m.is_dir())
                    .unwrap_or(true)
            {
                return Err(format!("refusing to remove unsafe path {}", path.display()));
            }
            let bytes = dir_size(&path);
            if !dry_run {
                fs::remove_dir_all(&path)
                    .map_err(|e| format!("cannot remove {}: {e}", path.display()))?;
            }
            if staging {
                report.removed_staging.push((name, bytes));
            } else {
                report.removed_versions.push((name, bytes));
            }
        }
    }
    report.kept.sort();
    Ok(report)
}

pub fn write_report(
    out: &mut impl Write,
    export: &Path,
    report: &PruneReport,
    dry_run: bool,
) -> std::io::Result<()> {
    let verb = if dry_run { "Would remove" } else { "Removed" };
    writeln!(out, "Export directory: {}", export.display())?;
    for name in &report.kept {
        writeln!(out, "Keep {name}")?;
    }
    for (name, bytes) in &report.removed_versions {
        writeln!(out, "{verb} snapshot {name} ({})", human(*bytes))?;
    }
    for (name, bytes) in &report.removed_staging {
        writeln!(out, "{verb} stale staging {name} ({})", human(*bytes))?;
    }
    for name in &report.skipped {
        writeln!(out, "Skipped unrecognized entry {name}")?;
    }
    writeln!(
        out,
        "{verb} {} snapshot(s) and {} staging director(ies); {} {}.",
        report.removed_versions.len(),
        report.removed_staging.len(),
        if dry_run {
            "would reclaim"
        } else {
            "reclaimed"
        },
        human(report.reclaimed_bytes())
    )
}

/// Parses `snapshots prune` arguments (after the two command words).
pub fn parse_prune_args(args: &[String]) -> Result<(usize, bool, Option<PathBuf>), String> {
    let (mut keep, mut dry, mut export) = (DEFAULT_KEEP, false, None);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dry-run" => dry = true,
            "--keep" => {
                i += 1;
                keep = args
                    .get(i)
                    .and_then(|v| v.parse().ok())
                    .ok_or("--keep requires a non-negative integer")?;
            }
            "--export" => {
                i += 1;
                export = Some(PathBuf::from(
                    args.get(i)
                        .filter(|v| !v.starts_with("--"))
                        .ok_or("--export requires a path")?,
                ));
            }
            other => return Err(format!("unknown snapshots prune argument: {other}")),
        }
        i += 1;
    }
    Ok((keep, dry, export))
}
