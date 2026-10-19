use std::fs::File;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Context;
use rusqlite::Connection;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Notify, broadcast};
use tp_core::audit::{AuditLog, NewEvent};
use tp_core::paths::FactoryPaths;
use tp_core::{Error, db, registry};
use tp_proto::{
    ActorKind, AuditTail, DaemonStatus, ErrorCode, Event, PROTOCOL_VERSION, ProjectAdd, Request, Response,
    event, method,
};

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
        Err(std::fs::TryLockError::Error(e)) => Err(e).with_context(|| format!("lock {}", path.display())),
    }
}

struct State {
    paths: FactoryPaths,
    conn: Mutex<Connection>,
    audit: AuditLog,
    started: Instant,
    started_at: String,
    events: broadcast::Sender<Event>,
    shutdown: Notify,
}

impl State {
    /// Records an audit event and pushes it to subscribers. Blocking.
    fn record(&self, ev: NewEvent) -> tp_core::Result<tp_proto::AuditRecord> {
        let rec = {
            let mut conn = self.conn.lock().map_err(|_| Error::Daemon("state lock poisoned".into()))?;
            self.audit.append(&mut conn, ev)?
        };
        let _ = self.events.send(Event { event: event::AUDIT_APPENDED.into(), data: json!(rec) });
        Ok(rec)
    }

    fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> tp_core::Result<T>) -> tp_core::Result<T> {
        let conn = self.conn.lock().map_err(|_| Error::Daemon("state lock poisoned".into()))?;
        f(&conn)
    }
}

pub async fn run(paths: FactoryPaths) -> anyhow::Result<()> {
    let conn = db::open(&paths.db())?;
    let (events, _) = broadcast::channel(256);
    let state = Arc::new(State {
        audit: AuditLog::new(&paths),
        conn: Mutex::new(conn),
        started: Instant::now(),
        started_at: tp_core::now_rfc3339(),
        events,
        shutdown: Notify::new(),
        paths,
    });

    let sock = state.paths.socket();
    // We hold the lock, so any existing socket file is stale.
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).with_context(|| format!("bind {}", sock.display()))?;
    std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600))?;

    let pid = std::process::id();
    blocking(&state, move |s| {
        s.record(NewEvent::system("daemon.started").details(json!({"pid": pid, "version": tp_core::VERSION})))
    })
    .await?;
    tracing::info!(socket = %sock.display(), pid, "tpd ready");

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
    let _ = std::fs::remove_file(&sock);
    blocking(&state, move |s| s.record(NewEvent::system("daemon.stopped").details(json!({"reason": reason}))))
        .await?;
    Ok(())
}

async fn blocking<T: Send + 'static>(
    state: &Arc<State>,
    f: impl FnOnce(&State) -> tp_core::Result<T> + Send + 'static,
) -> tp_core::Result<T> {
    let st = state.clone();
    tokio::task::spawn_blocking(move || f(&st))
        .await
        .unwrap_or_else(|e| Err(Error::Daemon(format!("worker panicked: {e}"))))
}

async fn serve(state: Arc<State>, stream: UnixStream) -> anyhow::Result<()> {
    let cred = stream.peer_cred().ok();
    let actor = cred.map_or_else(|| "local".to_string(), |c| format!("uid:{}", c.uid()));
    let (r, mut w) = stream.into_split();
    let mut reader = BufReader::new(r);
    let mut line = String::new();
    loop {
        line.clear();
        let n = (&mut reader).take(MAX_LINE as u64 + 1).read_line(&mut line).await?;
        if n == 0 {
            return Ok(());
        }
        if n > MAX_LINE {
            write(&mut w, &Response::err(0, ErrorCode::BadRequest, "request too large")).await?;
            return Ok(());
        }
        let req: Request = match serde_json::from_str(line.trim_end()) {
            Ok(r) => r,
            Err(e) => {
                write(&mut w, &Response::err(0, ErrorCode::BadRequest, format!("invalid request: {e}"))).await?;
                continue;
            }
        };
        if req.method == method::SUBSCRIBE {
            let mut rx = state.events.subscribe();
            write(&mut w, &Response::ok(req.id, json!({"subscribed": true}))).await?;
            return stream_events(&mut rx, &mut reader, &mut w).await;
        }
        let resp = dispatch(&state, &actor, req).await;
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

async fn write<T: serde::Serialize>(w: &mut tokio::net::unix::OwnedWriteHalf, msg: &T) -> anyhow::Result<()> {
    let mut buf = serde_json::to_vec(msg)?;
    buf.push(b'\n');
    w.write_all(&buf).await?;
    Ok(())
}

async fn dispatch(state: &Arc<State>, actor: &str, req: Request) -> Response {
    let id = req.id;
    let actor = actor.to_owned();
    let result: tp_core::Result<Value> = match req.method.as_str() {
        method::STATUS => blocking(state, |s| {
            let (projects, audit_events) = s.with_conn(|c| Ok((registry::count(c)?, s.audit.count(c)?)))?;
            Ok(json!(DaemonStatus {
                protocol: PROTOCOL_VERSION,
                version: tp_core::VERSION.into(),
                pid: std::process::id(),
                started_at: s.started_at.clone(),
                uptime_ms: s.started.elapsed().as_millis() as u64,
                factory_root: s.paths.root().display().to_string(),
                projects,
                audit_events,
            }))
        })
        .await,
        method::PROJECT_LIST => blocking(state, |s| Ok(json!(s.with_conn(registry::list)?))).await,
        method::PROJECT_ADD => match serde_json::from_value::<ProjectAdd>(req.params) {
            Ok(p) => {
                blocking(state, move |s| {
                    let project = s.with_conn(|c| registry::add(c, &s.paths, &p))?;
                    s.record(
                        NewEvent::new(ActorKind::Human, actor, "project.added")
                            .project(&project.slug)
                            .resource(&project.repo_path),
                    )?;
                    let _ = s.events.send(Event { event: event::PROJECT_ADDED.into(), data: json!(project) });
                    Ok(json!(project))
                })
                .await
            }
            Err(e) => Err(Error::Invalid(format!("project.add params: {e}"))),
        },
        method::AUDIT_TAIL => {
            let params = if req.params.is_null() { json!({}) } else { req.params };
            match serde_json::from_value::<AuditTail>(params) {
                Ok(t) => blocking(state, move |s| Ok(json!(s.with_conn(|c| s.audit.tail(c, t.limit.min(1000)))?)))
                    .await,
                Err(e) => Err(Error::Invalid(format!("audit.tail params: {e}"))),
            }
        }
        method::AUDIT_VERIFY => blocking(state, |s| Ok(json!(s.with_conn(|c| s.audit.verify(c))?))).await,
        method::SHUTDOWN => {
            let st = state.clone();
            let r = blocking(state, move |s| {
                s.record(NewEvent::new(ActorKind::Human, actor, "daemon.stop_requested"))?;
                Ok(json!({"stopping": true}))
            })
            .await;
            st.shutdown.notify_one();
            r
        }
        other => return Response::err(id, ErrorCode::UnknownMethod, format!("unknown method {other:?}")),
    };
    match result {
        Ok(v) => Response::ok(id, v),
        Err(e) => Response::err(id, code_for(&e), e.to_string()),
    }
}

fn code_for(e: &Error) -> ErrorCode {
    match e {
        Error::Invalid(_) | Error::Config { .. } => ErrorCode::InvalidParams,
        Error::NotFound(_) => ErrorCode::NotFound,
        Error::Conflict(_) => ErrorCode::Conflict,
        _ => ErrorCode::Internal,
    }
}
