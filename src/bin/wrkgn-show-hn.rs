use std::process::ExitCode;
use wrkgn::collector::HttpHnApi;
use wrkgn::show_hn;
use wrkgn::show_hn_discovery::HttpDiscovery;

fn main() -> ExitCode {
    let result = wrkgn::config::load_dotenv().and_then(|()| {
        show_hn::run(
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
