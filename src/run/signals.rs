//! Ctrl-C and SIGTERM handling. The first signal requests a clean stop at the
//! next item boundary; a second exits immediately with status 130.

use std::sync::Arc;

use super::control::RunControl;

/// Installs the process-wide handler. Call at most once per process.
pub fn install(control: Arc<RunControl>) -> Result<(), String> {
    ctrlc::set_handler(move || {
        if control.interrupt() {
            std::process::exit(130);
        }
    })
    .map_err(|error| format!("cannot install the interrupt handler: {error}"))
}
