use std::fs::File;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Notify, broadcast};
use tp_core::audit::NewEvent;
use tp_core::engine::Engine;
use tp_core::github::Gh;
use tp_core::paths::FactoryPaths;
use tp_core::runtime::{Claude, Runtime};
use tp_core::{Error, analytics, memory, portability, project_config, registry, store};
use tp_proto::*;

/// Requests larger than this are rejected (and the connection closed).
const MAX_LINE: usize = 1 << 20;

/// Holds the single-instance lock for the daemon's lifetime.
pub struct Lock(#[allow(dead_code)] File);

pub fn acquire_lock(paths: &FactoryPaths) -> anyhow::Result<Lock> {
    let path = paths.lock();
    let f = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    match f.try_lock() {
        Ok(()) => Ok(Lock(f)),
        Err(std::fs::TryLockError::WouldBlock) => anyhow::bail!(
            "another tpd is already running for {} (lock {})",
            paths.root().display(),
            path.display()
        ),
        Err(std::fs::TryLockError::Error(e)) => {
            Err(e).with_context(|| format!("lock {}", path.display()))
        }
    }
}

struct State {
    engine: Arc<Engine>,
    started: Instant,
    started_at: String,
    events: broadcast::Sender<Event>,
    shutdown: Notify,
}

/// `tp` binary used for hooks and the MCP server: $TP_BIN, next to tpd, then PATH.
fn find_tp() -> PathBuf {
    if let Ok(env) = std::env::var("TP_BIN") {
        return PathBuf::from(env);
    }
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("tp")))
        .filter(|p| p.is_file())
        .or_else(|| tp_core::doctor::which("tp"))
        .unwrap_or_else(|| PathBuf::from("tp"))
}

pub async fn run(paths: FactoryPaths) -> anyhow::Result<()> {
    let (events, _) = broadcast::channel::<Event>(1024);
    let tx = events.clone();
    let notify: tp_core::engine::Notify = Box::new(move |event, data| {
        let _ = tx.send(Event {
            event: event.into(),
            data,
        });
    });
    let cfg = tp_core::config::FactoryConfig::load(&paths.config())?;
    let runtime: Arc<dyn Runtime> = Arc::new(
        Claude::locate(cfg.runtime.claude_path.as_deref())
            .unwrap_or_else(|_| Claude::new("claude")),
    );
    let gh = Gh::cli(std::env::var_os("TP_GH").map(PathBuf::from));
    let engine = Engine::new(paths.clone(), runtime, gh, find_tp(), notify)?;
    let state = Arc::new(State {
        engine: engine.clone(),
        started: Instant::now(),
        started_at: tp_core::now_rfc3339(),
        events,
        shutdown: Notify::new(),
    });

    let sock = paths.socket();
    // We hold the lock, so any existing socket file is stale.
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).with_context(|| format!("bind {}", sock.display()))?;
    std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600))?;

    let pid = std::process::id();
    let e = engine.clone();
    tokio::task::spawn_blocking(move || -> tp_core::Result<()> {
        e.record(
            NewEvent::system("daemon.started")
                .details(json!({"pid": pid, "version": tp_core::VERSION})),
        )?;
        e.recover()?;
        Ok(())
    })
    .await??;
    tracing::info!(socket = %sock.display(), pid, "tpd ready");

    // Scheduler: one engine tick every few seconds on its own thread.
    let ticker = engine.clone();
    let stop_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop2 = stop_flag.clone();
    let tick_ms = std::env::var("TP_TICK_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3000u64);
    std::thread::spawn(move || {
        while !stop2.load(std::sync::atomic::Ordering::Relaxed) {
            ticker.tick();
            std::thread::sleep(Duration::from_millis(tick_ms));
        }
    });

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let reason = loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let st = state.clone();
                    tokio::spawn(async move {
                        if let Err(e) = serve(st, stream).await {
                            tracing::debug!("connection ended: {e:#}");
                        }
                    });
                }
                Err(e) => tracing::warn!("accept: {e}"),
            },
            _ = tokio::signal::ctrl_c() => break "sigint",
            _ = sigterm.recv() => break "sigterm",
            _ = state.shutdown.notified() => break "shutdown request",
        }
    };
    tracing::info!(reason, "tpd stopping");
    stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = std::fs::remove_file(&sock);
    let e = engine.clone();
    tokio::task::spawn_blocking(move || {
        e.record(NewEvent::system("daemon.stopped").details(json!({"reason": reason})))
    })
    .await??;
    Ok(())
}

async fn serve(state: Arc<State>, stream: UnixStream) -> anyhow::Result<()> {
    let cred = stream.peer_cred().ok();
    let actor = cred.map_or_else(|| "local".to_string(), |c| format!("uid:{}", c.uid()));
    let (r, mut w) = stream.into_split();
    let mut reader = BufReader::new(r);
    let mut line = String::new();
    loop {
        line.clear();
        let n = (&mut reader)
            .take(MAX_LINE as u64 + 1)
            .read_line(&mut line)
            .await?;
        if n == 0 {
            return Ok(());
        }
        if n > MAX_LINE {
            write(
                &mut w,
                &Response::err(0, ErrorCode::BadRequest, "request too large"),
            )
            .await?;
            return Ok(());
        }
        let req: Request = match serde_json::from_str(line.trim_end()) {
            Ok(r) => r,
            Err(e) => {
                write(
                    &mut w,
                    &Response::err(0, ErrorCode::BadRequest, format!("invalid request: {e}")),
                )
                .await?;
                continue;
            }
        };
        if req.method == method::SUBSCRIBE {
            let mut rx = state.events.subscribe();
            write(&mut w, &Response::ok(req.id, json!({"subscribed": true}))).await?;
            return stream_events(&mut rx, &mut reader, &mut w).await;
        }
        let id = req.id;
        if req.method == method::SHUTDOWN {
            let e = state.engine.clone();
            let a = actor.clone();
            let _ = tokio::task::spawn_blocking(move || {
                e.record(NewEvent::new(ActorKind::Human, a, "daemon.stop_requested"))
            })
            .await;
            write(&mut w, &Response::ok(id, json!({"stopping": true}))).await?;
            state.shutdown.notify_one();
            continue;
        }
        let st = state.clone();
        let a = actor.clone();
        let resp =
            match tokio::task::spawn_blocking(move || handle(&st, &a, &req.method, req.params))
                .await
            {
                Ok(Ok(v)) => Response::ok(id, v),
                Ok(Err(e)) => Response::err(id, e.0, e.1),
                Err(e) => Response::err(id, ErrorCode::Internal, format!("handler panicked: {e}")),
            };
        write(&mut w, &resp).await?;
    }
}

async fn stream_events(
    rx: &mut broadcast::Receiver<Event>,
    reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    w: &mut tokio::net::unix::OwnedWriteHalf,
) -> anyhow::Result<()> {
    let mut scratch = String::new();
    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(ev) => write(w, &ev).await?,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    write(w, &Event { event: "lagged".into(), data: json!({"missed": n}) }).await?;
                }
                Err(broadcast::error::RecvError::Closed) => return Ok(()),
            },
            // Any input (or EOF) on a subscribed connection ends it.
            _ = reader.read_line(&mut scratch) => return Ok(()),
        }
    }
}

async fn write<T: serde::Serialize>(
    w: &mut tokio::net::unix::OwnedWriteHalf,
    msg: &T,
) -> anyhow::Result<()> {
    let mut buf = serde_json::to_vec(msg)?;
    buf.push(b'\n');
    w.write_all(&buf).await?;
    Ok(())
}

type HErr = (ErrorCode, String);
type HResult = Result<Value, HErr>;

fn err(e: Error) -> HErr {
    let code = match &e {
        Error::Invalid(_) | Error::Config { .. } => ErrorCode::InvalidParams,
        Error::NotFound(_) => ErrorCode::NotFound,
        Error::Conflict(_) => ErrorCode::Conflict,
        _ => ErrorCode::Internal,
    };
    (code, e.to_string())
}

fn p<T: DeserializeOwned>(v: Value) -> Result<T, HErr> {
    let v = if v.is_null() { json!({}) } else { v };
    serde_json::from_value(v).map_err(|e| (ErrorCode::InvalidParams, format!("params: {e}")))
}

fn s(v: &Value, k: &str) -> Result<String, HErr> {
    v.get(k)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            (
                ErrorCode::InvalidParams,
                format!("missing string param `{k}`"),
            )
        })
}

fn opt(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_owned)
}

fn num(v: &Value, k: &str, default: u64) -> u64 {
    v.get(k).and_then(Value::as_u64).unwrap_or(default)
}

fn ok<T: serde::Serialize>(r: tp_core::Result<T>) -> HResult {
    r.map(|v| json!(v)).map_err(err)
}

fn handle(st: &State, actor: &str, method: &str, params: Value) -> HResult {
    let e = &st.engine;
    match method {
        method::STATUS => ok(e.db(|c| {
            Ok(DaemonStatus {
                protocol: PROTOCOL_VERSION,
                version: tp_core::VERSION.into(),
                pid: std::process::id(),
                started_at: st.started_at.clone(),
                uptime_ms: st.started.elapsed().as_millis() as u64,
                factory_root: e.paths.root().display().to_string(),
                projects: registry::count(c)?,
                audit_events: e.audit.count(c)?,
                mode: store::controls(c)?.mode,
                active_runs: store::active_runs(c)?.len() as u32,
                open_tasks: store::count_tasks(c, true)?,
                pending_approvals: store::count_pending_approvals(c)?,
                vault_unlocked: vault_unlocked(e),
            })
        })),
        method::PROJECT_LIST => ok(e.db(registry::list)),
        method::PROJECT_ADD => {
            let req: ProjectAdd = p(params)?;
            tp_core::doctor::can_access(
                std::path::Path::new(&req.repo_path),
                Duration::from_secs(5),
            )
            .map_err(|m| (ErrorCode::InvalidParams, m))?;
            let project = e.db(|c| registry::add(c, &e.paths, &req)).map_err(err)?;
            project_config::init_project_files(
                &e.paths,
                &project.slug,
                std::path::Path::new(&project.repo_path),
            )
            .map_err(err)?;
            e.record(
                NewEvent::new(ActorKind::Human, actor, "project.added")
                    .project(&project.slug)
                    .resource(&project.repo_path),
            )
            .map_err(err)?;
            e.emit(event::PROJECT_ADDED, json!(project));
            Ok(json!(project))
        }
        m::PROJECT_GET => ok(project_detail(e, &s(&params, "slug")?)),
        m::PROJECT_SET_CONFIG => {
            let slug = s(&params, "slug")?;
            e.db(|c| registry::get(c, &slug))
                .map_err(err)?
                .ok_or((ErrorCode::NotFound, format!("project {slug}")))?;
            if let Some(y) = opt(&params, "project_yaml") {
                project_config::parse_project(&y).map_err(err)?;
                project_config::write_atomic(&project_config::project_yaml(&e.paths, &slug), &y)
                    .map_err(err)?;
            }
            if let Some(y) = opt(&params, "verify_yaml") {
                project_config::parse_verify(&y).map_err(err)?;
                project_config::write_atomic(&project_config::verify_yaml(&e.paths, &slug), &y)
                    .map_err(err)?;
            }
            e.record(
                NewEvent::new(ActorKind::Human, actor, "project.config_changed").project(&slug),
            )
            .map_err(err)?;
            ok(project_detail(e, &slug))
        }
        method::AUDIT_TAIL => {
            let t: AuditTail = p(params)?;
            ok(e.db(|c| e.audit.tail(c, t.limit.min(5000))))
        }
        method::AUDIT_VERIFY => ok(e.db(|c| e.audit.verify(c))),

        m::TASK_LIST => {
            let q: TaskQuery = p(params)?;
            ok(e.db(|c| store::list_tasks(c, &q)))
        }
        m::TASK_GET => ok(e.task_detail(&s(&params, "id")?)),
        m::TASK_CREATE => {
            let t: TaskCreate = p(params)?;
            ok(e.create_task(&t, actor))
        }
        m::TASK_CANCEL => {
            ok(e.cancel_task(&s(&params, "id")?, opt(&params, "reason").as_deref(), actor))
        }
        m::TASK_RETRY => ok(e.retry_task(&s(&params, "id")?, actor)),
        m::TASK_DIFF => ok(e.task_diff(&s(&params, "id")?)),
        m::TASK_MESSAGE => ok(e.message_task(&s(&params, "id")?, &s(&params, "text")?, actor)),
        m::TASK_OPEN_TERMINAL => ok(open_terminal(e, &s(&params, "id")?)),

        m::RUN_EVENTS => {
            let n = store::parse_id('R', &s(&params, "run_id")?).map_err(err)?;
            let after = num(&params, "after_seq", 0);
            let limit = num(&params, "limit", 2000) as u32;
            ok(e.db(|c| store::run_events(c, n, after, limit)))
        }
        m::RUN_STOP => ok(e.stop_run(&s(&params, "run_id")?, actor)),

        m::PLAN_CURRENT => ok(e.db(store::latest_plan)),
        m::PLAN_GENERATE => ok(e.generate_plan(actor)),

        m::APPROVAL_LIST => {
            let state: Option<ApprovalState> = params
                .get("state")
                .filter(|v| !v.is_null())
                .map(|v| serde_json::from_value(v.clone()))
                .transpose()
                .map_err(|x| (ErrorCode::InvalidParams, x.to_string()))?;
            ok(e.db(|c| store::list_approvals(c, state, None)))
        }
        m::APPROVAL_DECIDE => {
            let d: ApprovalDecision = p(params)?;
            ok(e.decide(&d, actor))
        }

        m::SIGNAL_LIST => {
            let limit = num(&params, "limit", 200) as u32;
            ok(e.db(|c| store::list_signals(c, opt(&params, "project").as_deref(), limit)))
        }
        m::INTAKE_RECONCILE => ok(e
            .reconcile(opt(&params, "project").as_deref())
            .map(|(s, t)| json!({"signals_new": s, "tasks_created": t}))),

        m::FACTORY_CONTROLS => ok(e.controls()),
        m::FACTORY_SET_CONTROLS => {
            let u: ControlsUpdate = p(params)?;
            ok(e.set_controls(&u, actor))
        }
        m::FACTORY_STOP_ALL => ok(e.stop_all(actor).map(|n| json!({"stopped": n}))),

        m::DEPS_SCAN => ok(e.scan_deps(&s(&params, "project")?)),
        m::DEPS_LIST => ok(e.dep_items(opt(&params, "project").as_deref())),

        m::DEPLOY_LIST => {
            ok(e.db(|c| store::list_deployments(c, opt(&params, "project").as_deref(), None)))
        }
        m::DEPLOY_RUN => ok(manual_deploy(e, &params, actor)),

        m::MEMORY_LIST => ok(e.db(|c| {
            memory::list(
                c,
                opt(&params, "scope").as_deref(),
                opt(&params, "status").as_deref(),
            )
        })),
        m::MEMORY_SEARCH => {
            let q = s(&params, "query")?;
            ok(e.db(|c| memory::search(c, &q, opt(&params, "scope").as_deref(), true)))
        }
        m::MEMORY_PROPOSE => {
            let (scope, cat, text) = (
                s(&params, "scope")?,
                s(&params, "category")?,
                s(&params, "text")?,
            );
            let item = e
                .db(|c| {
                    memory::propose(
                        c,
                        &scope,
                        &cat,
                        &text,
                        json!({"author": actor, "source": "founder"}),
                        0.9,
                    )
                })
                .map_err(err)?
                .0;
            let _ = e.record(
                NewEvent::new(ActorKind::Human, actor, "memory.proposed")
                    .resource(item.id.to_string()),
            );
            Ok(json!(item))
        }
        m::MEMORY_SET_STATUS => {
            let id = params
                .get("id")
                .and_then(Value::as_i64)
                .ok_or((ErrorCode::InvalidParams, "missing integer `id`".to_string()))?;
            let status = s(&params, "status")?;
            let item = e.db(|c| memory::set_status(c, id, &status)).map_err(err)?;
            let _ = e.record(
                NewEvent::new(ActorKind::Human, actor, "memory.status")
                    .resource(id.to_string())
                    .details(json!({"status": status})),
            );
            Ok(json!(item))
        }

        m::USAGE_SUMMARY => {
            let days = num(&params, "days", 30) as u32;
            ok(e.db(|c| analytics::summary(c, days)))
        }
        m::BUDGET_LIST => ok(e.db(store::list_budgets)),
        m::BUDGET_SET => {
            let b: Budget = p(params)?;
            e.db(|c| store::set_budget(c, &b)).map_err(err)?;
            let _ =
                e.record(NewEvent::new(ActorKind::Human, actor, "budget.set").details(json!(b)));
            ok(e.db(store::list_budgets))
        }

        m::CHAT_HISTORY => {
            let limit = num(&params, "limit", 100) as u32;
            ok(e.db(|c| store::chat_history(c, limit)))
        }
        m::CHAT_SEND => ok(e.chat(&s(&params, "text")?, actor)),
        m::SEARCH => {
            let limit = num(&params, "limit", 30) as u32;
            ok(e.search(&s(&params, "query")?, limit))
        }

        m::VAULT_STATUS => ok(vault_status(e)),
        m::VAULT_INIT => {
            let pw = s(&params, "password")?;
            tp_vault::Vault::init(&e.paths.vault_dir(), &pw)
                .map_err(|x| (ErrorCode::InvalidParams, x.to_string()))?;
            let _ = e.record(NewEvent::new(ActorKind::Human, actor, "vault.initialized"));
            ok(vault_status(e))
        }
        m::VAULT_UNLOCK => {
            let pw = s(&params, "password")?;
            let ttl = num(&params, "ttl_minutes", 480).max(1);
            let vault = tp_vault::Vault::open(&e.paths.vault_dir())
                .map_err(|x| (ErrorCode::NotFound, x.to_string()))?;
            let unlocked = match vault.unlock(&pw) {
                Ok(u) => u,
                Err(x) => {
                    let _ = e.record(
                        NewEvent::new(ActorKind::Human, actor, "vault.unlock")
                            .outcome(Outcome::Denied),
                    );
                    return Err((ErrorCode::InvalidParams, x.to_string()));
                }
            };
            let ttl_d = Duration::from_secs(ttl * 60);
            let expires = time::OffsetDateTime::now_utc() + ttl_d;
            let holder = tp_vault::KeyHolder::new(
                unlocked,
                tp_vault::HolderConfig {
                    ttl: ttl_d,
                    idle: Some(Duration::from_secs(30 * 60)),
                },
                tp_vault::Lease::all(expires),
            );
            if let Ok(mut v) = e.vault.lock() {
                *v = Some(holder);
            }
            let _ = e.record(
                NewEvent::new(ActorKind::Human, actor, "vault.unlock")
                    .details(json!({"ttl_minutes": ttl})),
            );
            ok(vault_status(e))
        }
        m::VAULT_LOCK => {
            if let Ok(mut v) = e.vault.lock()
                && let Some(h) = v.take()
            {
                h.lock();
            }
            let _ = e.record(NewEvent::new(ActorKind::Human, actor, "vault.lock"));
            ok(vault_status(e))
        }
        m::VAULT_LIST => ok(vault_list(e, &params)),
        m::VAULT_SET => {
            let (proj, env, name, value) = (
                s(&params, "project")?,
                s(&params, "environment")?,
                s(&params, "name")?,
                s(&params, "value")?,
            );
            vault_write(e, |u| {
                u.set(&tp_vault::Scope::new(&proj, &env), &name, &value)
            })?;
            let _ = e.record(
                NewEvent::new(ActorKind::Human, actor, "vault.set")
                    .project(&proj)
                    .resource(format!("{proj}/{env}/{name}")),
            );
            let items =
                vault_list(e, &json!({"project": proj, "environment": env})).map_err(err)?;
            Ok(json!(items.into_iter().find(|i| i.name == name)))
        }
        m::VAULT_DELETE => {
            let (proj, env, name) = (
                s(&params, "project")?,
                s(&params, "environment")?,
                s(&params, "name")?,
            );
            vault_write(e, |u| u.delete(&tp_vault::Scope::new(&proj, &env), &name))?;
            let _ = e.record(
                NewEvent::new(ActorKind::Human, actor, "vault.delete")
                    .resource(format!("{proj}/{env}/{name}")),
            );
            Ok(json!({"deleted": true}))
        }

        m::EXPORT_CREATE => {
            let r: ExportRequest = p(params)?;
            let rep = e
                .db(|c| portability::export(&e.paths, c, &r))
                .map_err(err)?;
            let _ = e.record(
                NewEvent::new(ActorKind::Human, actor, "factory.exported")
                    .resource(&rep.path)
                    .details(json!({"include_vault": r.include_vault, "sha256": rep.sha256})),
            );
            Ok(json!(rep))
        }
        m::IMPORT_PREVIEW => {
            let path = PathBuf::from(s(&params, "path")?);
            ok(e.db(|c| portability::import(&e.paths, c, &path, false)))
        }
        m::IMPORT_APPLY => {
            let path = PathBuf::from(s(&params, "path")?);
            let rep = e
                .db(|c| portability::import(&e.paths, c, &path, true))
                .map_err(err)?;
            let _ = e.reload_config();
            let _ = e.record(
                NewEvent::new(ActorKind::Human, actor, "factory.imported")
                    .resource(&rep.path)
                    .details(json!({"projects": rep.projects, "memory_new": rep.memory_new})),
            );
            Ok(json!(rep))
        }
        m::DOCTOR => {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default();
            let mut checks = tp_core::doctor::run_all(&e.paths, &home);
            checks.retain(|c| c.name != "daemon");
            let worst = tp_core::doctor::worst(&checks);
            Ok(json!({"status": worst, "checks": checks}))
        }
        other => Err((
            ErrorCode::UnknownMethod,
            format!("unknown method {other:?}"),
        )),
    }
}

fn project_detail(e: &Engine, slug: &str) -> tp_core::Result<ProjectDetail> {
    let project = e
        .db(|c| registry::get(c, slug))?
        .ok_or_else(|| Error::NotFound(format!("project {slug}")))?;
    let pc = project_config::load_project(&e.paths, slug)?;
    let read = |p: PathBuf| std::fs::read_to_string(p).unwrap_or_default();
    let open = e.db(|c| {
        store::list_tasks(
            c,
            &TaskQuery {
                project: Some(slug.into()),
                open_only: true,
                ..Default::default()
            },
        )
    })?;
    Ok(ProjectDetail {
        github_repo: pc
            .github
            .clone()
            .or_else(|| tp_core::git::github_repo(std::path::Path::new(&project.repo_path))),
        base_branch: pc.base_branch.clone(),
        project_yaml: read(project_config::project_yaml(&e.paths, slug)),
        verify_yaml: read(project_config::verify_yaml(&e.paths, slug)),
        open_tasks: open.len() as u32,
        project,
    })
}

fn open_terminal(e: &Engine, id: &str) -> tp_core::Result<Value> {
    let d = e.task_detail(id)?;
    let wt = d
        .worktree
        .ok_or_else(|| Error::NotFound("task has no worktree yet".into()))?;
    let session = d.runs.iter().rev().find_map(|r| r.session_id.clone());
    let cmd = match session {
        Some(s) => format!("cd '{wt}' && claude --resume {s}"),
        None => format!("cd '{wt}'"),
    };
    let script = format!(
        "tell application \"Terminal\" to do script \"{}\"\ntell application \"Terminal\" to activate",
        cmd.replace('\\', "\\\\").replace('"', "\\\"")
    );
    let opened = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .status()
        .is_ok_and(|s| s.success());
    let _ = e.record(NewEvent::new(ActorKind::Human, "founder", "task.terminal_opened").task(id));
    Ok(json!({"opened": opened}))
}

fn manual_deploy(e: &Arc<Engine>, params: &Value, actor: &str) -> tp_core::Result<Deployment> {
    let project = params
        .get("project")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Invalid("missing project".into()))?;
    let envname = params
        .get("environment")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Invalid("missing environment".into()))?;
    let p = e
        .db(|c| registry::get(c, project))?
        .ok_or_else(|| Error::NotFound(format!("project {project}")))?;
    let pc = project_config::load_project(&e.paths, project)?;
    let env = pc
        .deploy
        .iter()
        .find(|d| d.name == envname)
        .ok_or_else(|| Error::NotFound(format!("environment {envname} in project.yaml")))?
        .clone();
    if e.controls()?.mode == Mode::Safe {
        return Err(Error::Invalid(
            "factory is in safe mode; deployments are disabled".into(),
        ));
    }
    let repo = PathBuf::from(&p.repo_path);
    let commit = tp_core::git::head_sha(&repo)?;
    let auth = format!("approval:founder:{actor}");
    let dn = e.db(|c| store::insert_deployment(c, project, &env.name, None, &commit, &auth))?;
    let secrets = e.secret_env(project, env.secrets.as_deref().unwrap_or(&env.name));
    let r = tp_core::deploy::deploy(&repo, &env, &commit, &secrets, Duration::from_secs(60));
    let d = e.db(|c| {
        store::finish_deployment(
            c,
            dn,
            &r.state,
            r.health.as_deref(),
            r.version_ok,
            r.rolled_back,
        )
    })?;
    e.record(
        NewEvent::new(ActorKind::Human, actor, "deploy.finished")
            .project(project)
            .resource(&env.name)
            .authorization(&auth)
            .outcome(if r.state == "healthy" {
                Outcome::Ok
            } else {
                Outcome::Failed
            })
            .details(json!(d)),
    )?;
    Ok(d)
}

fn vault_unlocked(e: &Engine) -> bool {
    e.vault
        .lock()
        .ok()
        .and_then(|v| v.as_ref().map(|h| h.is_unlocked()))
        .unwrap_or(false)
}

fn vault_status(e: &Engine) -> tp_core::Result<VaultStatus> {
    let dir = e.paths.vault_dir();
    let initialized = tp_vault::Vault::is_initialized(&dir);
    let (unlocked, expires, items) = match e.vault.lock() {
        Ok(v) => match v.as_ref() {
            Some(h) if h.is_unlocked() => {
                let st = h.status();
                let items = h.with_unlocked(|u| u.list(None).len()).unwrap_or(0);
                (true, st.expires_at.map(|t| t.to_string()), items as u32)
            }
            _ => (false, None, 0),
        },
        Err(_) => (false, None, 0),
    };
    Ok(VaultStatus {
        initialized,
        unlocked,
        lease_expires_at: expires,
        items,
    })
}

fn vault_list(e: &Engine, params: &Value) -> tp_core::Result<Vec<VaultItemMeta>> {
    let v = e
        .vault
        .lock()
        .map_err(|_| Error::Daemon("vault lock".into()))?;
    let h = v
        .as_ref()
        .filter(|h| h.is_unlocked())
        .ok_or_else(|| Error::Invalid("vault is locked; unlock it first".into()))?;
    let items = h
        .with_unlocked(|u| u.list(None))
        .ok_or_else(|| Error::Invalid("vault is locked".into()))?;
    let want_p = opt(params, "project");
    let want_e = opt(params, "environment");
    Ok(items
        .into_iter()
        .filter(|i| want_p.as_ref().is_none_or(|p| &i.scope.project == p))
        .filter(|i| want_e.as_ref().is_none_or(|x| &i.scope.environment == x))
        .map(|i| VaultItemMeta {
            project: i.scope.project.clone(),
            environment: i.scope.environment.clone(),
            name: i.name,
            version: i.version as u32,
            updated_at: i.updated_at.to_string(),
            expires_at: i.expires_at.map(|t| t.to_string()),
            rotation_due: i.rotation_due,
        })
        .collect())
}

fn vault_write(
    e: &Engine,
    f: impl FnOnce(&mut tp_vault::Unlocked) -> tp_vault::Result<()>,
) -> Result<(), HErr> {
    let v = e
        .vault
        .lock()
        .map_err(|_| (ErrorCode::Internal, "vault lock".to_string()))?;
    let h = v.as_ref().filter(|h| h.is_unlocked()).ok_or((
        ErrorCode::InvalidParams,
        "vault is locked; unlock it first".to_string(),
    ))?;
    h.with_unlocked(f)
        .ok_or((ErrorCode::InvalidParams, "vault is locked".to_string()))?
        .map_err(|x| (ErrorCode::InvalidParams, x.to_string()))
}
