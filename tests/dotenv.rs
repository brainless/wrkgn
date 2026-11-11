use std::fs;
use std::process::Command;
use tempfile::tempdir;

fn cli(dir: &std::path::Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wrkgn"));
    command.current_dir(dir).env_clear().args(args);
    command
}

#[test]
fn dotenv_sets_paths_before_cli_dispatch() {
    let directory = tempdir().unwrap();
    fs::write(
        directory.path().join(".env"),
        "WRKGN_DB_PATH=private/db.sqlite\nWRKGN_EXPORT_PATH=public\n",
    )
    .unwrap();
    let result = cli(directory.path(), &["export"]).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(directory.path().join("private/db.sqlite").exists());
    assert!(directory.path().join("public/current.json").exists());
}

#[test]
fn shell_values_override_dotenv() {
    let directory = tempdir().unwrap();
    fs::write(
        directory.path().join(".env"),
        "WRKGN_DB_PATH=from-file/db.sqlite\nWRKGN_EXPORT_PATH=public\n",
    )
    .unwrap();
    let result = cli(directory.path(), &["export"])
        .env("WRKGN_DB_PATH", "from-shell/db.sqlite")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(directory.path().join("from-shell/db.sqlite").exists());
    assert!(!directory.path().join("from-file/db.sqlite").exists());
}

#[test]
fn missing_file_is_allowed_and_malformed_file_reports_no_secret() {
    let directory = tempdir().unwrap();
    assert!(
        cli(directory.path(), &["--help"])
            .output()
            .unwrap()
            .status
            .success()
    );
    fs::write(
        directory.path().join(".env"),
        "OPENAI_API_KEY=private-token\ninvalid line with spaces\n",
    )
    .unwrap();
    let result = cli(directory.path(), &["--help"]).output().unwrap();
    assert!(!result.status.success());
    let error = String::from_utf8(result.stderr).unwrap();
    assert!(error.contains("malformed .env"));
    assert!(!error.contains("private-token"));
}
