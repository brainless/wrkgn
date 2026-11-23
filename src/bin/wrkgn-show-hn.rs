use std::io::IsTerminal;
use std::process::ExitCode;
use wrkgn::collector::{CollectionConfig, Collector, HttpHnApi};
use wrkgn::processing::RigModel;
use wrkgn::run::status::{StatusLine, SystemClock};
use wrkgn::run::{self, signals};
use wrkgn::show_hn::{self, Args};
use wrkgn::show_hn_discovery::HttpDiscovery;
use wrkgn::store::Store;

fn main() -> ExitCode {
    match real_main() {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(1)
        }
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

fn real_main() -> Result<u8, String> {
    wrkgn::config::load_dotenv()?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help") {
        println!("{}", show_hn::HELP);
        return Ok(0);
    }
    let args = Args::parse(&args)?;
    let model = RigModel::from_env()?;
    let db = args.db.clone().unwrap_or_else(Store::configured_path);
    let mut store = Store::open(&db).map_err(|e| e.to_string())?;
    // The lock is held until the session drops at the end of this function.
    let session = run::begin(&mut store, &db, "show_hn", unix_now())?;
    signals::install(session.control.clone())?;
    let collector = Collector::new(HttpHnApi::default(), CollectionConfig::default())
        .with_control(session.control.clone());
    let discovery = HttpDiscovery::default().with_control(session.control.clone());
    let stderr = std::io::stderr();
    let mut status = StatusLine::new(std::io::stderr(), stderr.is_terminal(), SystemClock);
    let result = show_hn::execute_driver(
        &mut store,
        &session,
        &collector,
        &model,
        &discovery,
        &args,
        &unix_now,
        &mut status,
        &mut std::io::stderr(),
    );
    Ok(result.exit_code)
}
