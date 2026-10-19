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
import { inTauri, invoke } from "./transport";
import { api, daemonStatus, useTpdConnection, useTpdEvents } from "./api";
import { Icon, Logo } from "./icons";
import type { Link, Project, TaskSummary } from "./types";
import {
  AppContext,
  href,
  navigate,
  useHashRoute,
  VIEWS,
  type AlertEntry,
  type AppCtx,
  type Route,
  type ViewName,
} from "./router";
import { fmtUptime, isActiveState, Loading, taskTone, toast, Toaster } from "./ui";

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
const SIDEBAR_KEY = "tp.sidebar";
const ALERT_LIMIT = 50;
const RECENT_LIMIT = 12;
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

function readStore(key: string): string {
  try {
    return localStorage.getItem(key) ?? "";
  } catch {
    return "";
  }
}
function writeStore(key: string, v: string) {
  try {
    localStorage.setItem(key, v);
  } catch {
    /* storage unavailable: lasts for this session */
  }
}

/** Views that manage their own full-height layout. */
const isFlush = (r: Route) => (r.view === "tasks" && !!r.id) || r.view === "chat";

export default function App() {
  const route = useHashRoute();
  const [starting, setStarting] = useState(false);
  const [link, setLink] = useState<Link>({ state: "connecting" });
  const [projects, setProjects] = useState<Project[]>([]);
  const [project, setProjectState] = useState(() => readStore(PROJECT_KEY));
  const [alerts, setAlerts] = useState<AlertEntry[]>([]);
  const [recent, setRecent] = useState<TaskSummary[]>([]);
  const [palette, setPalette] = useState<null | "search" | "project">(null);
  const [help, setHelp] = useState(false);
  const [collapsed, setCollapsed] = useState(() => readStore(SIDEBAR_KEY) === "collapsed");
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

  const loadLists = useCallback(() => {
    api.projectList().then(
      (p) => setProjects([...p].sort((a, b) => a.slug.localeCompare(b.slug))),
      () => undefined,
    );
    api.taskList({ limit: 50 }).then(
      (t) => setRecent([...t].sort((a, b) => b.updated_at.localeCompare(a.updated_at)).slice(0, RECENT_LIMIT)),
      () => undefined,
    );
  }, []);

  useEffect(() => {
    if (!readyReported) {
      readyReported = true;
      void invoke("app_ready").catch(() => undefined);
    }
    void refresh();
    loadLists();
    // Warm the ⌘K palette chunk once the shell is idle.
    const idle = window.setTimeout(() => void loadPalette().catch(() => undefined), 1500);
    // Status/uptime only; live data arrives as events.
    const timer = window.setInterval(() => void refresh(), 15000);
    return () => {
      window.clearInterval(timer);
      window.clearTimeout(idle);
    };
  }, [refresh, loadLists]);

  useTpdConnection((online, reason) => {
    if (online) {
      void refresh();
      loadLists();
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
    if (ev.event === "task.updated") {
      const t = ev.data;
      setRecent((prev) => [t, ...prev.filter((x) => x.id !== t.id)].slice(0, RECENT_LIMIT));
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
    writeStore(PROJECT_KEY, slug);
  }, []);

  const toggleSidebar = useCallback(() => {
    setCollapsed((c) => {
      writeStore(SIDEBAR_KEY, c ? "open" : "collapsed");
      return !c;
    });
  }, []);

  const newTask = useCallback(() => {
    navigate({ view: "overview" });
    // Home focuses its composer on this event (also after navigation).
    window.setTimeout(() => window.dispatchEvent(new Event("tp:new-task")), 30);
  }, []);

  useShortcuts({
    openPalette: (mode) => setPalette(mode),
    toggleHelp: () => setHelp((h) => !h),
    toggleSidebar,
    newTask,
    closeAll: () => {
      setPalette(null);
      setHelp(false);
    },
  });

  const startFactory = () => {
    setStarting(true);
    invoke<string>("install_daemon")
      .then(() => {
        toast("Factory started.");
        return refresh();
      })
      .catch((e) => toast(`Could not start tpd: ${String(e)}`, "bad"))
      .finally(() => setStarting(false));
  };

  const ctx: AppCtx = useMemo(
    () => ({ link, project, setProject, projects, refreshStatus: () => void refresh(), alerts }),
    [link, project, setProject, projects, refresh, alerts],
  );

  const status = link.state === "online" ? link.status : null;
  const View = VIEW_COMPONENTS[route.view];
  const current = VIEWS.find((v) => v.name === route.view);
  const navItem = (v: (typeof VIEWS)[number]) => {
    const on = route.view === v.name;
    return (
      <li key={v.name}>
        <a
          href={`#/${v.name}`}
          className={on ? "nav active" : "nav"}
          aria-current={on ? "page" : undefined}
          title={`${v.label} (g ${v.key})`}
        >
          <Icon name={v.icon} />
          <span className="nav-text">{v.label}</span>
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
    );
  };

  return (
    <AppContext.Provider value={ctx}>
      <div className={`app ${collapsed ? "collapsed" : ""} ${inTauri ? "native" : ""}`}>
        <a className="skip" href="#main">
          Skip to content
        </a>
        <nav className="sidebar" aria-label="Primary">
          <div className="side-top" data-tauri-drag-region style={inTauri ? undefined : { paddingLeft: 10 }}>
            <button className="icon-btn" onClick={toggleSidebar} title="Hide sidebar (⌘\)" aria-label="Hide sidebar">
              <Icon name="sidebar" />
            </button>
          </div>
          <div className="brand">
            <span className="logo" aria-hidden>
              <Logo />
            </span>
            Thee Pori
          </div>
          <div className="side-actions">
            <button className="nav new-task" onClick={newTask} title="New task (⌘N)">
              <Icon name="edit" />
              <span className="nav-text">New task</span>
            </button>
            <button className="search-btn" onClick={() => setPalette("search")} title="Search (⌘K)">
              <Icon name="search" />
              <span>Search</span>
              <kbd>⌘K</kbd>
            </button>
          </div>
          <div className="side-scroll">
            <ul>{VIEWS.filter((v) => v.group === "main").map(navItem)}</ul>
            <div className="side-label">Factory</div>
            <ul>{VIEWS.filter((v) => v.group === "factory").map(navItem)}</ul>
            {recent.length > 0 && (
              <>
                <div className="side-label">Recent tasks</div>
                <ul>
                  {recent.map((t) => {
                    const on = route.view === "tasks" && route.id === t.id;
                    const live = isActiveState(t.state);
                    return (
                      <li key={t.id}>
                        <a
                          href={href({ view: "tasks", id: t.id })}
                          className={`nav recent ${on ? "active" : ""}`}
                          title={`${t.title} · ${t.project} · ${t.state.replace(/_/g, " ")}`}
                        >
                          <span className={`dot ${live ? "live" : taskTone(t.state)}`} />
                          <span className="nav-text">{t.title}</span>
                        </a>
                      </li>
                    );
                  })}
                </ul>
              </>
            )}
          </div>
          <div className="sidebar-foot">
            <button
              className="daemon"
              onClick={() => navigate({ view: "settings" })}
              title={status ? `tpd ${status.version} · pid ${status.pid} · ${status.factory_root}` : link.state === "offline" ? link.reason : "connecting"}
            >
              <span className={`dot ${status ? (status.mode === "normal" ? "ok" : status.mode === "safe" ? "bad" : "warn") : link.state === "offline" ? "bad" : ""}`} />
              <span style={{ minWidth: 0 }}>
                {status ? (status.mode === "normal" ? "Factory running" : status.mode === "safe" ? "Safe mode" : "Paused") : link.state === "offline" ? "Factory offline" : "Connecting…"}
                <small>{status ? `tpd ${status.version} · up ${fmtUptime(status.uptime_ms)}` : "tpd"}</small>
              </span>
            </button>
            <button className="icon-btn" onClick={() => setHelp(true)} title="Keyboard shortcuts (?)" aria-label="Keyboard shortcuts">
              <Icon name="keyboard" />
            </button>
          </div>
        </nav>

        <div className="main-col">
          <header className="topbar" data-tauri-drag-region>
            {collapsed && (
              <button className="icon-btn" onClick={toggleSidebar} title="Show sidebar (⌘\)" aria-label="Show sidebar">
                <Icon name="sidebar" />
              </button>
            )}
            <span className="crumb" data-tauri-drag-region>
              {route.view === "tasks" && route.id ? (
                <>
                  <a href="#/tasks">Tasks</a>
                  <Icon name="chevronRight" size={13} className="muted" />
                  <span className="mono">{route.id}</span>
                </>
              ) : route.view === "projects" && route.id ? (
                <>
                  <a href="#/projects">Projects</a>
                  <Icon name="chevronRight" size={13} className="muted" />
                  <span>{route.id}</span>
                </>
              ) : route.view === "overview" ? null : (
                current?.label
              )}
            </span>
            <div className="topbar-right">
              {status && status.mode !== "normal" && (
                <button className={`pill ${status.mode === "safe" ? "bad" : "warn"}`} onClick={() => navigate({ view: "settings" })}>
                  <Icon name={status.mode === "safe" ? "shield" : "pause"} size={13} />
                  {status.mode === "safe" ? "Safe mode" : "Paused"}
                </button>
              )}
              {status && status.pending_approvals > 0 && route.view !== "approvals" && (
                <button className="pill accent" onClick={() => navigate({ view: "approvals" })}>
                  <Icon name="inbox" size={13} />
                  {status.pending_approvals} awaiting you
                </button>
              )}
              <label className="scope">
                <span className="sr-only">Project scope</span>
                <select value={project} onChange={(e) => setProject(e.target.value)} aria-label="Project scope" title="Project scope (⌘P)">
                  <option value="">All projects</option>
                  {projects.map((p) => (
                    <option key={p.slug} value={p.slug}>
                      {p.slug}
                    </option>
                  ))}
                </select>
              </label>
            </div>
          </header>

          {link.state === "offline" && (
            <div className="offline-banner" role="alert">
              <span className="dot bad" />
              <span className="grow">
                <strong>The factory is not running</strong>
                <span className="muted small">
                  Agents only work while tpd runs in the background. {link.reason && `(${link.reason})`}
                </span>
              </span>
              <button className="btn small ghost" onClick={() => void refresh()}>
                Retry
              </button>
              {inTauri && (
                <button className="btn small accent" disabled={starting} onClick={startFactory}>
                  {starting ? "Starting…" : "Start factory"}
                </button>
              )}
            </div>
          )}

          <main id="main" tabIndex={-1} className={isFlush(route) ? "flush" : ""}>
            <ViewBoundary key={route.view}>
              <Suspense fallback={<Loading what="Loading" />}>
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
        <Toaster />
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
  toggleSidebar: () => void;
  newTask: () => void;
  closeAll: () => void;
}) {
  const ref = useRef(h);
  ref.current = h;
  useEffect(() => {
    let gAt = 0;
    const onKey = (e: KeyboardEvent) => {
      const meta = e.metaKey || e.ctrlKey;
      const k = e.key.toLowerCase();
      if (meta && k === "k") {
        e.preventDefault();
        ref.current.openPalette("search");
        return;
      }
      if (meta && k === "p") {
        e.preventDefault();
        ref.current.openPalette("project");
        return;
      }
      if (meta && k === "n") {
        e.preventDefault();
        ref.current.newTask();
        return;
      }
      if (meta && e.key === "\\") {
        e.preventDefault();
        ref.current.toggleSidebar();
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
  const rows: [ReactNode, string][] = [
    [<kbd key="n">⌘N</kbd>, "New task"],
    [
      <>
        <kbd>⌘K</kbd> or <kbd>/</kbd>
      </>,
      "Search everything",
    ],
    [<kbd key="p">⌘P</kbd>, "Switch project scope"],
    [<kbd key="b">⌘\</kbd>, "Show or hide the sidebar"],
    ...VIEWS.map(
      (v) =>
        [
          <>
            <kbd>g</kbd> <kbd>{v.key}</kbd>
          </>,
          v.label,
        ] as [ReactNode, string],
    ),
    [<kbd key="q">?</kbd>, "Toggle this help"],
    [<kbd key="e">Esc</kbd>, "Close dialogs"],
  ];
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
            {rows.map(([k, l], i) => (
              <tr key={i}>
                <td>{k}</td>
                <td>{l}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
