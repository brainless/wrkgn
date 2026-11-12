use std::process::ExitCode;
use wrkgn::collector::HttpHnApi;
use wrkgn::monthly::{self, HttpDiscovery};

fn main() -> ExitCode {
    let result = wrkgn::config::load_dotenv().and_then(|()| {
        monthly::run(
            &std::env::args().skip(1).collect::<Vec<_>>(),
            HttpHnApi::default(),
            HttpDiscovery::default(),
            &mut std::io::stdout(),
        )
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
