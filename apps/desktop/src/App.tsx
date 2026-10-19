import {
  Component,
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ComponentType,
  type ReactNode,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { api, daemonStatus, useTpdConnection, useTpdEvents } from "./api";
import type { Link, Project } from "./types";
import {
  AppContext,
  navigate,
  useHashRoute,
  VIEWS,
  type AlertEntry,
  type AppCtx,
  type Route,
  type ViewName,
} from "./router";
import { fmtUptime, Loading } from "./ui";

// Views are code-split so the first commit only needs the shell.
const VIEW_COMPONENTS: Record<ViewName, ComponentType<{ route: Route }>> = {
  overview: lazy(() => import("./views/Overview")),
  plan: lazy(() => import("./views/Plan")),
  approvals: lazy(() => import("./views/Approvals")),
  tasks: lazy(() => import("./views/Tasks")),
  projects: lazy(() => import("./views/Projects")),
  chat: lazy(() => import("./views/Chat")),
  memory: lazy(() => import("./views/Memory")),
  usage: lazy(() => import("./views/Usage")),
  vault: lazy(() => import("./views/Vault")),
  settings: lazy(() => import("./views/Settings")),
};
const loadPalette = () => import("./Palette");
const Palette = lazy(loadPalette);

const PROJECT_KEY = "tp.project";
const ALERT_LIMIT = 50;
const STATUS_EVENTS = [
  "task.updated",
  "run.updated",
  "approval.created",
  "approval.decided",
  "controls.changed",
  "project.added",
] as const;

// The shell's first DOM commit is "usable shell" for the perf harness
// (requestAnimationFrame never fires for an occluded window).
let readyReported = false;

function readScope(): string {
  try {
    return localStorage.getItem(PROJECT_KEY) ?? "";
  } catch {
    return "";
  }
}

export default function App() {
  const route = useHashRoute();
  const [starting, setStarting] = useState(false);
  const [link, setLink] = useState<Link>({ state: "connecting" });
  const [projects, setProjects] = useState<Project[]>([]);
  const [project, setProjectState] = useState(readScope);
  const [alerts, setAlerts] = useState<AlertEntry[]>([]);
  const [palette, setPalette] = useState<null | "search" | "project">(null);
  const [help, setHelp] = useState(false);
  const busy = useRef(false);
  const alertSeq = useRef(0);

  const refresh = useCallback(async () => {
    if (busy.current) return;
    busy.current = true;
    try {
      const status = await daemonStatus();
      if (!status) {
        setLink({ state: "offline", reason: "tpd is not running" });
        return;
      }
      setLink({ state: "online", status });
    } catch (e) {
      setLink({ state: "offline", reason: String(e) });
    } finally {
      busy.current = false;
    }
  }, []);

  const loadProjects = useCallback(() => {
    api.projectList().then(
      (p) => setProjects([...p].sort((a, b) => a.slug.localeCompare(b.slug))),
      () => undefined,
    );
  }, []);

  useEffect(() => {
    if (!readyReported) {
      readyReported = true;
      void invoke("app_ready").catch(() => undefined);
    }
    void refresh();
    loadProjects();
    // Warm the ⌘K palette chunk once the shell is idle.
    const idle = window.setTimeout(() => void loadPalette().catch(() => undefined), 1500);
    // Status/uptime only; live data arrives as events.
    const timer = window.setInterval(() => void refresh(), 15000);
    return () => {
      window.clearInterval(timer);
      window.clearTimeout(idle);
    };
  }, [refresh, loadProjects]);

  useTpdConnection((online, reason) => {
    if (online) {
      void refresh();
      loadProjects();
    } else {
      setLink({ state: "offline", reason: reason ?? "tpd disconnected" });
    }
  });

  // Counters in the status (pending approvals, active runs) follow events,
  // coalesced so a burst of task updates costs one status call.
  const statusTimer = useRef(0);
  useTpdEvents((ev) => {
    if (ev.event === "project.added") {
      const p = ev.data;
      setProjects((prev) =>
        prev.some((x) => x.slug === p.slug) ? prev : [...prev, p].sort((a, b) => a.slug.localeCompare(b.slug)),
      );
    }
    window.clearTimeout(statusTimer.current);
    statusTimer.current = window.setTimeout(() => void refresh(), 400);
  }, STATUS_EVENTS);

  useTpdEvents(
    (ev) => {
      if (ev.event !== "alert") return;
      const entry = { id: ++alertSeq.current, ts: new Date().toISOString(), alert: ev.data };
      setAlerts((prev) => [entry, ...prev].slice(0, ALERT_LIMIT));
    },
    ["alert"],
  );

  const setProject = useCallback((slug: string) => {
    setProjectState(slug);
    try {
      localStorage.setItem(PROJECT_KEY, slug);
    } catch {
      /* storage unavailable: scope lasts for this session */
    }
  }, []);

  useShortcuts({
    openPalette: (mode) => setPalette(mode),
    toggleHelp: () => setHelp((h) => !h),
    closeAll: () => {
      setPalette(null);
      setHelp(false);
    },
  });

  const ctx: AppCtx = useMemo(
    () => ({ link, project, setProject, projects, refreshStatus: () => void refresh(), alerts }),
    [link, project, setProject, projects, refresh, alerts],
  );

  const status = link.state === "online" ? link.status : null;
  const View = VIEW_COMPONENTS[route.view];
  const current = VIEWS.find((v) => v.name === route.view);

  return (
    <AppContext.Provider value={ctx}>
      <div className="app">
        <a className="skip" href="#main">
          Skip to content
        </a>
        <nav className="sidebar" aria-label="Primary">
          <div className="brand">
            <span className="logo" aria-hidden>
              ◆
            </span>
            Thee Pori
          </div>
          <ul>
            {VIEWS.map((v) => (
              <li key={v.name}>
                <a
                  href={`#/${v.name}`}
                  className={route.view === v.name ? "nav active" : "nav"}
                  aria-current={route.view === v.name ? "page" : undefined}
                  title={`g ${v.key}`}
                >
                  <span>{v.label}</span>
                  {v.name === "approvals" && status && status.pending_approvals > 0 && (
                    <span className="nav-count" aria-label={`${status.pending_approvals} pending`}>
                      {status.pending_approvals}
                    </span>
                  )}
                  {v.name === "tasks" && status && status.active_runs > 0 && (
                    <span className="nav-count info" aria-label={`${status.active_runs} active runs`}>
                      {status.active_runs}
                    </span>
                  )}
                </a>
              </li>
            ))}
          </ul>
          <div className="sidebar-foot">
            <button className="linkish" onClick={() => setHelp(true)}>
              Keyboard shortcuts <kbd>?</kbd>
            </button>
            {status && (
              <span className="muted small">
                tpd {status.version} · up {fmtUptime(status.uptime_ms)}
              </span>
            )}
          </div>
        </nav>

        <div className="main-col">
          <header className="topbar">
            <span className="crumb">{current?.label}</span>
            <label className="scope">
              <span className="sr-only">Project scope</span>
              <select
                value={project}
                onChange={(e) => setProject(e.target.value)}
                aria-label="Project scope"
                title="Project scope (⌘P)"
              >
                <option value="">All projects</option>
                {projects.map((p) => (
                  <option key={p.slug} value={p.slug}>
                    {p.slug}
                  </option>
                ))}
              </select>
            </label>
            <button className="search-btn" onClick={() => setPalette("search")} aria-label="Search (⌘K)">
              <span>Search…</span>
              <kbd>⌘K</kbd>
            </button>
            <div className="topbar-right">
              {status && status.pending_approvals > 0 && (
                <button className="pill warn" onClick={() => navigate({ view: "approvals" })}>
                  {status.pending_approvals} pending approval{status.pending_approvals === 1 ? "" : "s"}
                </button>
              )}
              {status && status.mode !== "normal" && (
                <button className={`pill ${status.mode === "safe" ? "bad" : "warn"}`} onClick={() => navigate({ view: "settings" })}>
                  {status.mode === "safe" ? "Safe mode" : "Paused"}
                </button>
              )}
              <DaemonBadge link={link} />
            </div>
          </header>

          {link.state === "offline" && (
            <div className="offline-banner" role="alert">
              <strong>tpd offline</strong> — work is not running from this window; start with{" "}
              <code>tp daemon install</code>
              <span className="muted"> ({link.reason})</span>
              <button
                className="btn small primary"
                disabled={starting}
                onClick={() => {
                  setStarting(true);
                  invoke<string>("install_daemon")
                    .then(() => refresh())
                    .catch((e) => window.alert(`Could not start tpd:\n${String(e)}`))
                    .finally(() => setStarting(false));
                }}
              >
                {starting ? "Starting…" : "Start factory"}
              </button>
              <button className="btn small" onClick={() => void refresh()}>
                Retry
              </button>
            </div>
          )}

          <main id="main" tabIndex={-1}>
            <ViewBoundary key={route.view}>
              <Suspense fallback={<Loading what="Loading view" />}>
                <View route={route} />
              </Suspense>
            </ViewBoundary>
          </main>
        </div>

        {palette && (
          <Suspense fallback={null}>
            <Palette mode={palette} onClose={() => setPalette(null)} />
          </Suspense>
        )}
        {help && <HelpOverlay onClose={() => setHelp(false)} />}
      </div>
    </AppContext.Provider>
  );
}

/** Keeps one broken view from blanking the whole window. */
class ViewBoundary extends Component<{ children: ReactNode }, { error: string | null }> {
  state = { error: null as string | null };
  static getDerivedStateFromError(e: unknown) {
    return { error: e instanceof Error ? e.message : String(e) };
  }
  render() {
    if (this.state.error)
      return (
        <div className="note bad" role="alert">
          <span>This view failed to render: {this.state.error}</span>
          <button className="btn small" onClick={() => this.setState({ error: null })}>
            Try again
          </button>
        </div>
      );
    return this.props.children;
  }
}

function DaemonBadge({ link }: { link: Link }) {
  if (link.state === "online") {
    const s = link.status;
    return (
      <span className="badge online" title={`tpd ${s.version} · pid ${s.pid} · ${s.factory_root}`}>
        tpd online · {s.mode}
      </span>
    );
  }
  return <span className={`badge ${link.state}`}>{link.state === "offline" ? "tpd offline" : "connecting"}</span>;
}

// ------------------------------------------------------------ shortcuts

function isTyping(t: EventTarget | null): boolean {
  const el = t as HTMLElement | null;
  if (!el) return false;
  const tag = el.tagName;
  return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || el.isContentEditable;
}

function useShortcuts(h: {
  openPalette: (mode: "search" | "project") => void;
  toggleHelp: () => void;
  closeAll: () => void;
}) {
  const ref = useRef(h);
  ref.current = h;
  useEffect(() => {
    let gAt = 0;
    const onKey = (e: KeyboardEvent) => {
      const meta = e.metaKey || e.ctrlKey;
      if (meta && e.key.toLowerCase() === "k") {
        e.preventDefault();
        ref.current.openPalette("search");
        return;
      }
      if (meta && e.key.toLowerCase() === "p") {
        e.preventDefault();
        ref.current.openPalette("project");
        return;
      }
      if (e.key === "Escape") {
        ref.current.closeAll();
        return;
      }
      if (meta || e.altKey || isTyping(e.target)) return;
      if (document.querySelector("[aria-modal='true']")) return;
      if (e.key === "?") {
        e.preventDefault();
        ref.current.toggleHelp();
        return;
      }
      if (e.key === "/") {
        e.preventDefault();
        ref.current.openPalette("search");
        return;
      }
      if (e.key === "g") {
        gAt = Date.now();
        return;
      }
      if (gAt && Date.now() - gAt < 1200) {
        gAt = 0;
        const v = VIEWS.find((x) => x.key === e.key);
        if (v) {
          e.preventDefault();
          navigate({ view: v.name });
          document.getElementById("main")?.focus();
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}

function HelpOverlay({ onClose }: { onClose: () => void }) {
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    box.current?.focus();
  }, []);
  return (
    <div className="overlay" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-label="Keyboard shortcuts"
        tabIndex={-1}
        ref={box}
        onKeyDown={(e) => (e.key === "Escape" || e.key === "?") && onClose()}
      >
        <div className="modal-head">
          <h2>Keyboard shortcuts</h2>
          <button className="modal-x" aria-label="Close" onClick={onClose}>
            ×
          </button>
        </div>
        <table className="shortcuts">
          <tbody>
            <tr>
              <td>
                <kbd>⌘K</kbd> or <kbd>/</kbd>
              </td>
              <td>Search everything</td>
            </tr>
            <tr>
              <td>
                <kbd>⌘P</kbd>
              </td>
              <td>Switch project scope</td>
            </tr>
            {VIEWS.map((v) => (
              <tr key={v.name}>
                <td>
                  <kbd>g</kbd> <kbd>{v.key}</kbd>
                </td>
                <td>{v.label}</td>
              </tr>
            ))}
            <tr>
              <td>
                <kbd>?</kbd>
              </td>
              <td>Toggle this help</td>
            </tr>
            <tr>
              <td>
                <kbd>Esc</kbd>
              </td>
              <td>Close dialogs</td>
            </tr>
          </tbody>
        </table>
      </div>
    </div>
  );
}
