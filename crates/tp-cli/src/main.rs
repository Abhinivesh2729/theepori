//! `tp`: command-line control for the Thee Pori software factory.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use serde_json::{Value, json};
use tp_core::audit::{AuditLog, NewEvent};
use tp_core::client::Client;
use tp_core::doctor::{self, Status};
use tp_core::paths::FactoryPaths;
use tp_core::{db, launchd, registry, setup};
use tp_proto::{ActorKind, AuditRecord, AuditVerify, DaemonStatus, Project, ProjectAdd, method};

#[derive(Parser)]
#[command(name = "tp", version = tp_core::VERSION, about = "Thee Pori software factory")]
struct Cli {
    /// Factory directory (default: $TP_FACTORY or ~/factory)
    #[arg(long, global = true, value_name = "DIR")]
    factory: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create or repair the factory (safe to re-run)
    Setup(JsonFlag),
    /// Check the factory and its environment
    Doctor(JsonFlag),
    /// Manage registered projects
    #[command(subcommand)]
    Project(ProjectCmd),
    /// Control the tpd daemon
    #[command(subcommand)]
    Daemon(DaemonCmd),
    /// Read and verify the audit log
    #[command(subcommand)]
    Audit(AuditCmd),
    /// Claude Code hook entrypoint (used by tp-generated settings)
    #[command(hide = true)]
    Hook {
        /// pre | post
        event: String,
        #[arg(long)]
        run: String,
    },
    /// MCP server for agent runs (used by tp-generated MCP config)
    #[command(hide = true)]
    Mcp {
        #[arg(long)]
        run: String,
    },
    /// Import secrets from a Panuval store into TP Vault
    VaultImportPanuval {
        /// Panuval store path (default ~/.config/panuval)
        #[arg(long)]
        store: Option<PathBuf>,
    },
}

#[derive(Args)]
struct JsonFlag {
    /// Print JSON
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum ProjectCmd {
    /// Register a local git checkout
    Add {
        path: PathBuf,
        #[arg(long)]
        slug: Option<String>,
        #[arg(long)]
        name: Option<String>,
        /// Jira project key, e.g. SF
        #[arg(long)]
        jira: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// List registered projects
    List(JsonFlag),
}

#[derive(Subcommand)]
enum DaemonCmd {
    /// Install tpd as a launchd user agent and start it
    Install {
        /// Path to the tpd binary (default: next to tp, then PATH)
        #[arg(long)]
        tpd: Option<PathBuf>,
        #[arg(long, default_value = launchd::DEFAULT_LABEL, hide = true)]
        label: String,
    },
    /// Stop tpd and remove the launchd agent
    Uninstall {
        #[arg(long, default_value = launchd::DEFAULT_LABEL, hide = true)]
        label: String,
    },
    /// Show whether tpd is running
    Status(JsonFlag),
    /// Run tpd in the foreground
    Run {
        #[arg(long)]
        tpd: Option<PathBuf>,
    },
    /// Ask a running tpd to stop
    Stop,
    /// Measure IPC round-trip latency against a running tpd
    Bench {
        #[arg(short, default_value_t = 1000)]
        n: usize,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum AuditCmd {
    /// Show the latest audit records
    Tail {
        #[arg(short, default_value_t = 20)]
        n: u32,
        #[arg(long)]
        json: bool,
    },
    /// Recompute the hash chain
    Verify(JsonFlag),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("tp: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    let paths = match cli.factory {
        Some(p) => FactoryPaths::new(p),
        None => FactoryPaths::resolve()?,
    };
    match cli.cmd {
        Cmd::Setup(f) => {
            let r = setup::setup(&paths, &actor())?;
            if f.json {
                print_json(&r)?;
            } else if r.changed() {
                println!("Factory ready at {}", paths.root().display());
                for c in &r.created {
                    println!("  created {c}");
                }
                if r.migrations_applied > 0 {
                    println!("  applied {} database migration(s)", r.migrations_applied);
                }
            } else {
                println!("Factory at {} is up to date.", paths.root().display());
            }
        }
        Cmd::Doctor(f) => {
            let checks = doctor::run_all(&paths, &home()?);
            let worst = doctor::worst(&checks);
            if f.json {
                print_json(&json!({"status": worst, "checks": checks}))?;
            } else {
                for c in &checks {
                    let mark = match c.status {
                        Status::Ok => "ok  ",
                        Status::Warn => "warn",
                        Status::Fail => "FAIL",
                    };
                    println!("{mark}  {:<22} {}", c.name, c.detail);
                    if let Some(fix) = c.fix.as_ref().filter(|_| c.status != Status::Ok) {
                        println!("      {:<22} fix: {fix}", "");
                    }
                }
            }
            if worst == Status::Fail {
                return Ok(ExitCode::FAILURE);
            }
        }
        Cmd::Project(ProjectCmd::Add {
            path,
            slug,
            name,
            jira,
            json,
        }) => {
            let req = ProjectAdd {
                repo_path: path.to_string_lossy().into_owned(),
                slug,
                name,
                jira_key: jira,
            };
            let p: Project = match daemon(&paths) {
                Some(mut c) => c.call(method::PROJECT_ADD, serde_json::to_value(&req)?)?,
                None => add_direct(&paths, &req)?,
            };
            if json {
                print_json(&p)?;
            } else {
                println!("Registered {} ({}) -> {}", p.slug, p.name, p.repo_path);
            }
        }
        Cmd::Project(ProjectCmd::List(f)) => {
            let list: Vec<Project> = match daemon(&paths) {
                Some(mut c) => c.call(method::PROJECT_LIST, Value::Null)?,
                None => registry::list(&open_db(&paths)?)?,
            };
            if f.json {
                print_json(&list)?;
            } else if list.is_empty() {
                println!("No projects. Add one with `tp project add <path>`.");
            } else {
                for p in list {
                    println!(
                        "{:<20} {:<6} {}",
                        p.slug,
                        p.jira_key.as_deref().unwrap_or("-"),
                        p.repo_path
                    );
                }
            }
        }
        Cmd::Daemon(cmd) => return daemon_cmd(&paths, cmd),
        Cmd::Hook { event, run } => {
            use std::io::Read;
            let mut input = String::new();
            let _ = std::io::stdin().read_to_string(&mut input);
            match event.as_str() {
                "pre" => {
                    let out = tp_core::hook::pre(&paths, &run, &input);
                    if !out.is_null() {
                        println!("{out}");
                    }
                }
                "post" => {
                    let _ = tp_core::hook::post(&paths, &run, &input);
                }
                other => bail!("unknown hook event {other}"),
            }
        }
        Cmd::Mcp { run } => {
            let stdin = std::io::stdin();
            tp_core::mcp::serve(&paths, &run, stdin.lock(), std::io::stdout())?;
        }
        Cmd::VaultImportPanuval { store } => {
            let store = store.unwrap_or_else(|| home().unwrap_or_default().join(".config/panuval"));
            let vault = tp_vault::Vault::open(&paths.vault_dir()).context(
                "TP Vault is not initialized; run `tp` desktop Vault > Initialize first",
            )?;
            let tp_pw = rpassword_prompt("TP Vault master password: ")?;
            let mut unlocked = vault.unlock(&tp_pw)?;
            let pan_pw = rpassword_prompt("Panuval master password: ")?;
            let report = tp_vault::import_panuval(&store, &pan_pw, &mut unlocked, |i| {
                let project = i
                    .folder
                    .rsplit('/')
                    .next()
                    .map(registry::slugify)
                    .filter(|f| !f.is_empty());
                tp_vault::Scope::new(project.unwrap_or_else(|| "global".into()), "default")
            })?;
            println!(
                "Imported {} item(s), skipped {}.",
                report.imported.len(),
                report.skipped.len()
            );
            for s in &report.skipped {
                println!("  skipped {}: {}", s.panuval_path, s.reason);
            }
        }
        Cmd::Audit(AuditCmd::Tail { n, json }) => {
            let recs: Vec<AuditRecord> = match daemon(&paths) {
                Some(mut c) => c.call(method::AUDIT_TAIL, json!({"limit": n}))?,
                None => AuditLog::new(&paths).tail(&open_db(&paths)?, n)?,
            };
            if json {
                print_json(&recs)?;
            } else {
                for r in recs {
                    println!(
                        "{:>6}  {}  {:<24} {:?}:{}  {}",
                        r.seq,
                        r.ts,
                        r.action,
                        r.actor.kind,
                        r.actor.id,
                        r.project.as_deref().unwrap_or("")
                    );
                }
            }
        }
        Cmd::Audit(AuditCmd::Verify(f)) => {
            let v: AuditVerify = match daemon(&paths) {
                Some(mut c) => c.call(method::AUDIT_VERIFY, Value::Null)?,
                None => AuditLog::new(&paths).verify(&open_db(&paths)?)?,
            };
            if f.json {
                print_json(&v)?;
            } else if v.ok {
                println!("Audit chain intact: {} records.", v.records);
            } else {
                println!(
                    "Audit chain BROKEN after record {}: {}",
                    v.records,
                    v.problem.unwrap_or_default()
                );
            }
            if !v.ok {
                return Ok(ExitCode::FAILURE);
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn daemon_cmd(paths: &FactoryPaths, cmd: DaemonCmd) -> Result<ExitCode> {
    match cmd {
        DaemonCmd::Install { tpd, label } => {
            let tpd = find_tpd(tpd)?;
            setup::setup(paths, &actor())?;
            let plist = launchd::install(&home()?, &label, &tpd, paths)?;
            let deadline = Instant::now() + Duration::from_secs(10);
            while daemon(paths).is_none() {
                if Instant::now() > deadline {
                    bail!(
                        "launchd loaded {} but tpd is not answering; see {}",
                        plist.display(),
                        paths.logs().join("tpd.log").display()
                    );
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            println!("tpd installed ({}) and running.", plist.display());
        }
        DaemonCmd::Uninstall { label } => {
            if launchd::uninstall(&home()?, &label)? {
                println!("tpd stopped and launchd agent removed.");
            } else {
                println!("No launchd agent installed.");
            }
        }
        DaemonCmd::Status(f) => match daemon(paths) {
            Some(mut c) => {
                let s: DaemonStatus = c.call(method::STATUS, Value::Null)?;
                if f.json {
                    print_json(&s)?;
                } else {
                    println!(
                        "tpd {} running (pid {}, up {}s) for {}: {} project(s), {} audit record(s)",
                        s.version,
                        s.pid,
                        s.uptime_ms / 1000,
                        s.factory_root,
                        s.projects,
                        s.audit_events
                    );
                }
            }
            None => {
                if f.json {
                    print_json(&json!({"running": false}))?;
                } else {
                    println!("tpd is not running. Start it with `tp daemon install`.");
                }
                return Ok(ExitCode::from(3));
            }
        },
        DaemonCmd::Run { tpd } => {
            let tpd = find_tpd(tpd)?;
            let status = std::process::Command::new(&tpd)
                .args(["--factory", &paths.root().to_string_lossy()])
                .status()
                .with_context(|| format!("run {}", tpd.display()))?;
            return Ok(ExitCode::from(status.code().unwrap_or(1) as u8));
        }
        DaemonCmd::Stop => match daemon(paths) {
            Some(mut c) => {
                c.call_raw(method::SHUTDOWN, Value::Null)?;
                println!(
                    "tpd is stopping. (Under launchd it stays stopped until `tp daemon install`.)"
                );
            }
            None => println!("tpd is not running."),
        },
        DaemonCmd::Bench { n, json } => {
            let mut c = daemon(paths).context("tpd is not running")?;
            let mut samples = Vec::with_capacity(n);
            for _ in 0..n.max(1) {
                let t = Instant::now();
                c.call_raw(method::STATUS, Value::Null)?;
                samples.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            samples.sort_by(f64::total_cmp);
            let pct = |p: f64| samples[((samples.len() as f64 - 1.0) * p).round() as usize];
            let r = json!({"method": "status", "n": samples.len(), "p50_ms": pct(0.5), "p95_ms": pct(0.95), "p99_ms": pct(0.99), "max_ms": pct(1.0)});
            if json {
                print_json(&r)?;
            } else {
                println!(
                    "status x{}: p50 {:.3} ms, p95 {:.3} ms, p99 {:.3} ms",
                    samples.len(),
                    pct(0.5),
                    pct(0.95),
                    pct(0.99)
                );
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Mutations without a daemon go straight to the core, still audited.
fn add_direct(paths: &FactoryPaths, req: &ProjectAdd) -> Result<Project> {
    let mut conn = open_db(paths)?;
    let p = registry::add(&conn, paths, req)?;
    tp_core::project_config::init_project_files(
        paths,
        &p.slug,
        std::path::Path::new(&p.repo_path),
    )?;
    AuditLog::new(paths).append(
        &mut conn,
        NewEvent::new(ActorKind::Human, actor(), "project.added")
            .project(&p.slug)
            .resource(&p.repo_path),
    )?;
    Ok(p)
}

fn open_db(paths: &FactoryPaths) -> Result<db::Connection> {
    if !paths.db().exists() {
        bail!(
            "no factory at {}; run `tp setup` first",
            paths.root().display()
        );
    }
    let mut conn = db::open(&paths.db())?;
    if db::version(&conn)? < db::latest_version() {
        db::migrate(&mut conn)?;
    }
    Ok(conn)
}

fn daemon(paths: &FactoryPaths) -> Option<Client> {
    if !paths.socket().exists() {
        return None;
    }
    let mut c = Client::connect(&paths.socket(), Duration::from_secs(10)).ok()?;
    c.call_raw(method::STATUS, Value::Null).ok()?;
    Some(c)
}

fn find_tpd(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return p.canonicalize().with_context(|| format!("{}", p.display()));
    }
    if let Some(sibling) = std::env::current_exe()?
        .parent()
        .map(|d| d.join("tpd"))
        .filter(|p| p.is_file())
    {
        return Ok(sibling);
    }
    doctor::which("tpd")
        .context("tpd not found next to tp or on PATH; build it with `make build` or pass --tpd")
}

/// Reads a password from the terminal without echo (stty), falling back to a plain read.
fn rpassword_prompt(prompt: &str) -> Result<String> {
    use std::io::Write;
    eprint!("{prompt}");
    std::io::stderr().flush().ok();
    let echo_off = std::process::Command::new("stty")
        .arg("-echo")
        .stdin(std::process::Stdio::inherit())
        .status()
        .is_ok_and(|s| s.success());
    let mut s = String::new();
    std::io::stdin().read_line(&mut s)?;
    if echo_off {
        let _ = std::process::Command::new("stty")
            .arg("echo")
            .stdin(std::process::Stdio::inherit())
            .status();
        eprintln!();
    }
    Ok(s.trim_end_matches(['\n', '\r']).to_string())
}

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")
}

fn actor() -> String {
    std::env::var("USER")
        .map(|u| format!("cli:{u}"))
        .unwrap_or_else(|_| "cli".into())
}

fn print_json<T: serde::Serialize>(v: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(())
}
