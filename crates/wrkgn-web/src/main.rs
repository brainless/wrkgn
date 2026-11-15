use wrkgn_web::{
    config::{Config, HELP},
    server,
};
#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|s| s == "--help" || s == "-h") {
        println!("{HELP}");
        return;
    }
    let result = if args.first().is_some_and(|s| s == "check") {
        if args.len() != 2 {
            Err("usage: wrkgn-web check <snapshot-root-or-public.sqlite>".into())
        } else {
            wrkgn_public_snapshot::open_validated(std::path::Path::new(&args[1])).map(
                |(_, metadata)| {
                    println!(
                        "valid public snapshot schema={} version={}",
                        metadata.schema_version, metadata.version
                    )
                },
            )
        }
    } else {
        match Config::parse(&args) {
            Ok(config) => server::serve(config).await,
            Err(error) => Err(error),
        }
    };
    if let Err(error) = result {
        eprintln!("wrkgn-web: {error}");
        std::process::exit(1);
    }
}
