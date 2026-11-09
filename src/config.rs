//! Local environment configuration, loaded before CLI dispatch.
use std::path::Path;

/// Load a complete `.env` file from the current working directory. Explicit
/// process environment values take precedence. Parsing completes before any
/// values are installed, so a malformed file cannot partially configure a run.
pub fn load_dotenv() -> Result<(), String> {
    load_dotenv_from(Path::new(".env"))
}

fn load_dotenv_from(path: &Path) -> Result<(), String> {
    let iterator = match dotenvy::from_path_iter(path) {
        Ok(iterator) => iterator,
        Err(dotenvy::Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(());
        }
        Err(_) => return Err("could not read .env".into()),
    };
    let entries: Vec<_> = iterator
        .collect::<Result<_, _>>()
        .map_err(|_| "malformed .env file".to_string())?;
    for (key, value) in entries {
        if std::env::var_os(&key).is_none() {
            // Called once at process startup, before collectors create threads.
            unsafe { std::env::set_var(key, value) };
        }
    }
    Ok(())
}
