//! tpd: the Thee Pori factory daemon. Owns all factory work; the desktop app,
//! CLI and (later) the relay are clients of its Unix socket.

mod server;

use std::path::PathBuf;
use std::process::ExitCode;

use tp_core::paths::FactoryPaths;

/// Exit code when another tpd already holds the factory lock (EX_TEMPFAIL).
const EXIT_LOCKED: u8 = 75;
/// Exit code for an unusable factory or config (EX_CONFIG).
const EXIT_CONFIG: u8 = 78;

fn main() -> ExitCode {
    let mut factory: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--factory" => factory = args.next().map(PathBuf::from),
            "--version" | "-V" => {
                println!("tpd {}", tp_core::VERSION);
                return ExitCode::SUCCESS;
            }
            "--help" | "-h" => {
                println!(
                    "usage: tpd [--factory <dir>]\n\nRuns the factory daemon in the foreground. Install it as a launchd agent with `tp daemon install`."
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("tpd: unknown argument {other:?}; see `tpd --help`");
                return ExitCode::from(2);
            }
        }
    }
    tracing_subscriber::fmt()
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();

    let paths = match factory
        .map(FactoryPaths::new)
        .map_or_else(FactoryPaths::resolve, Ok)
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("tpd: {e}");
            return ExitCode::from(EXIT_CONFIG);
        }
    };
    if let Err(e) = tp_core::setup::setup(&paths, "tpd") {
        eprintln!(
            "tpd: factory at {} is not usable: {e}",
            paths.root().display()
        );
        return ExitCode::from(EXIT_CONFIG);
    }
    let lock = match server::acquire_lock(&paths) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("tpd: {e}");
            return ExitCode::from(EXIT_LOCKED);
        }
    };

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("tpd: cannot start runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = rt.block_on(server::run(paths));
    drop(lock);
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("tpd: {e:#}");
            ExitCode::FAILURE
        }
    }
}
