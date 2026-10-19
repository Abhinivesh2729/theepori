//! Thee Pori desktop app: a client of tpd. Closing it never stops factory work.
//!
//! All socket IO runs on blocking worker threads; the UI only receives
//! command results and pushed `tpd-*` events.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::Value;
use tauri::{AppHandle, Emitter, State};
use tp_core::client::Client;
use tp_core::paths::FactoryPaths;
use tp_proto::{AuditRecord, DaemonStatus, Project, method};

static STARTED: OnceLock<Instant> = OnceLock::new();

struct Socket(PathBuf);

fn connect(sock: &PathBuf) -> Result<Client, String> {
    Client::connect(sock, Duration::from_secs(5)).map_err(|e| e.to_string())
}

async fn on_worker<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
}

/// `None` when tpd is not running (not an error for the UI).
#[tauri::command]
async fn daemon_status(sock: State<'_, Socket>) -> Result<Option<DaemonStatus>, String> {
    let sock = sock.0.clone();
    on_worker(move || match connect(&sock) {
        Err(_) => Ok(None),
        Ok(mut c) => c
            .call(method::STATUS, Value::Null)
            .map(Some)
            .map_err(|e| e.to_string()),
    })
    .await
}

#[tauri::command]
async fn project_list(sock: State<'_, Socket>) -> Result<Vec<Project>, String> {
    let sock = sock.0.clone();
    on_worker(move || {
        connect(&sock)?
            .call(method::PROJECT_LIST, Value::Null)
            .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
async fn audit_tail(sock: State<'_, Socket>, limit: u32) -> Result<Vec<AuditRecord>, String> {
    let sock = sock.0.clone();
    on_worker(move || {
        connect(&sock)?
            .call(method::AUDIT_TAIL, serde_json::json!({ "limit": limit }))
            .map_err(|e| e.to_string())
    })
    .await
}

/// Called by the UI after its first paint. With `TP_PERF_OUT` set, writes the
/// time to usable shell for the perf harness; with `TP_PERF_EXIT=1`, quits.
#[tauri::command]
fn app_ready(app: AppHandle) {
    let ms = STARTED
        .get()
        .map_or(0.0, |t| t.elapsed().as_secs_f64() * 1000.0);
    if let Ok(out) = std::env::var("TP_PERF_OUT") {
        let _ = std::fs::write(out, format!("{{\"ready_ms\":{ms:.1}}}\n"));
    }
    if std::env::var("TP_PERF_EXIT").is_ok_and(|v| v == "1") {
        app.exit(0);
    }
}

/// Keeps one subscription to tpd open and forwards its events to the UI,
/// reconnecting every second while tpd is down.
fn bridge_events(app: AppHandle, sock: PathBuf) {
    std::thread::spawn(move || {
        let mut online: Option<bool> = None;
        loop {
            let reason = match connect(&sock).and_then(|c| {
                c.set_timeout(None).map_err(|e| e.to_string())?;
                c.subscribe().map_err(|e| e.to_string())
            }) {
                Ok(events) => {
                    online = Some(true);
                    let _ = app.emit("tpd-connected", ());
                    let mut reason = "tpd closed the connection".to_string();
                    for ev in events {
                        match ev {
                            Ok(ev) => {
                                let _ = app.emit("tpd-event", ev);
                            }
                            Err(e) => {
                                reason = e.to_string();
                                break;
                            }
                        }
                    }
                    reason
                }
                Err(_) => "tpd is not running".to_string(),
            };
            if online != Some(false) {
                online = Some(false);
                let _ = app.emit("tpd-disconnected", reason);
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
}

fn main() {
    let _ = STARTED.set(Instant::now());
    let paths = FactoryPaths::resolve().unwrap_or_else(|_| FactoryPaths::new("factory"));
    let sock = paths.socket();
    tauri::Builder::default()
        .manage(Socket(sock.clone()))
        .invoke_handler(tauri::generate_handler![
            daemon_status,
            project_list,
            audit_tail,
            app_ready
        ])
        .setup(move |app| {
            bridge_events(app.handle().clone(), sock.clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .unwrap_or_else(|e| {
            eprintln!("Thee Pori desktop failed to start: {e}");
            std::process::exit(1);
        });
}
