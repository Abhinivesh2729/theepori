import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { AuditRecord, DaemonStatus, Link, Project, TpdEvent } from "./types";

const AUDIT_LIMIT = 100;

// The shell's first DOM commit is "usable shell" for the perf harness
// (requestAnimationFrame never fires for an occluded window).
let readyReported = false;

export default function App() {
  const [link, setLink] = useState<Link>({ state: "connecting" });
  const [projects, setProjects] = useState<Project[]>([]);
  const [audit, setAudit] = useState<AuditRecord[]>([]);
  const busy = useRef(false);

  const refresh = useCallback(async () => {
    if (busy.current) return;
    busy.current = true;
    try {
      const status = await invoke<DaemonStatus | null>("daemon_status");
      if (!status) {
        setLink({ state: "offline", reason: "tpd is not running" });
        return;
      }
      setLink({ state: "online", status });
      const [p, a] = await Promise.all([
        invoke<Project[]>("project_list"),
        invoke<AuditRecord[]>("audit_tail", { limit: AUDIT_LIMIT }),
      ]);
      setProjects(p);
      setAudit(a.reverse());
    } catch (e) {
      setLink({ state: "offline", reason: String(e) });
    } finally {
      busy.current = false;
    }
  }, []);

  useEffect(() => {
    if (!readyReported) {
      readyReported = true;
      void invoke("app_ready").catch(() => undefined);
    }
    void refresh();
    const unlisten = [
      listen<TpdEvent>("tpd-event", (msg) => {
        const ev = msg.payload;
        if (ev.event === "audit.appended") {
          setAudit((prev) => [ev.data as AuditRecord, ...prev].slice(0, AUDIT_LIMIT));
        }
        if (ev.event === "project.added") {
          setProjects((prev) =>
            [...prev, ev.data as Project].sort((a, b) => a.slug.localeCompare(b.slug)),
          );
        }
      }),
      listen("tpd-connected", () => void refresh()),
      listen<string>("tpd-disconnected", (msg) =>
        setLink({ state: "offline", reason: msg.payload }),
      ),
    ];
    // Status/uptime only; live data arrives as events.
    const timer = window.setInterval(() => void refresh(), 15000);
    return () => {
      window.clearInterval(timer);
      unlisten.forEach((p) => void p.then((f) => f()));
    };
  }, [refresh]);

  return (
    <div className="shell">
      <header>
        <h1>Thee Pori</h1>
        <span className="sub">Mission Control</span>
        <DaemonBadge link={link} />
      </header>
      <main>
        <section className="card">
          <h2>Factory</h2>
          <DaemonDetails link={link} />
        </section>
        <section className="card">
          <h2>Projects <span className="count">{projects.length}</span></h2>
          {projects.length === 0 ? (
            <p className="muted">
              No projects yet. Register one with <code>tp project add &lt;path&gt; --jira KEY</code>.
            </p>
          ) : (
            <table>
              <thead>
                <tr><th>Slug</th><th>Jira</th><th>Repository</th></tr>
              </thead>
              <tbody>
                {projects.map((p) => (
                  <tr key={p.slug}>
                    <td>{p.slug}</td>
                    <td>{p.jira_key ?? "–"}</td>
                    <td className="path">{p.repo_path}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </section>
        <section className="card wide">
          <h2>Audit <span className="count">{audit.length}</span></h2>
          <ol className="audit">
            {audit.map((r) => (
              <li key={r.seq} className={r.outcome}>
                <span className="seq">#{r.seq}</span>
                <time>{r.ts.replace("T", " ").replace("Z", "")}</time>
                <span className="action">{r.action}</span>
                <span className="actor">{r.actor.kind}:{r.actor.id}</span>
                {r.project && <span className="project">{r.project}</span>}
                {r.provenance === "claimed" && <span className="claimed">claimed</span>}
              </li>
            ))}
          </ol>
        </section>
      </main>
    </div>
  );
}

function DaemonBadge({ link }: { link: Link }) {
  const label = link.state === "online" ? "tpd online" : link.state === "offline" ? "tpd offline" : "connecting";
  return <span className={`badge ${link.state}`}>{label}</span>;
}

function DaemonDetails({ link }: { link: Link }) {
  if (link.state === "connecting") return <p className="muted">Connecting to tpd…</p>;
  if (link.state === "offline")
    return (
      <div>
        <p>{link.reason}</p>
        <p className="muted">
          Start it with <code>tp daemon install</code>. Work never depends on this window being open.
        </p>
      </div>
    );
  const s = link.status;
  return (
    <dl>
      <dt>Root</dt><dd className="path">{s.factory_root}</dd>
      <dt>Daemon</dt><dd>tpd {s.version} · pid {s.pid} · up {formatUptime(s.uptime_ms)}</dd>
      <dt>Projects</dt><dd>{s.projects}</dd>
      <dt>Audit records</dt><dd>{s.audit_events}</dd>
    </dl>
  );
}

function formatUptime(ms: number): string {
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  return `${Math.floor(m / 60)}h ${m % 60}m`;
}
