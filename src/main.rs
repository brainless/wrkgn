use std::process::ExitCode;

use wrkgn::cli::run_with_api;
use wrkgn::collector::HttpHnApi;

fn main() -> ExitCode {
    if let Err(error) = wrkgn::config::load_dotenv() {
        eprintln!("configuration error: {error}");
        return ExitCode::FAILURE;
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run_with_api(
        &args,
        HttpHnApi::default(),
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    ) {
        Ok(status) => ExitCode::from(status),
        Err(error) => {
            eprintln!("output error: {error}");
            ExitCode::FAILURE
        }
    }
}
