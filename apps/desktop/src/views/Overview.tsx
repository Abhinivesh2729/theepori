// Home: start a task in one line, then what needs you and what is running.

import { useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import { api, useAction, useQuery, useTpdEvents } from "../api";
import { ApprovalCard } from "../ApprovalCard";
import { Icon, Logo } from "../icons";
import { href, navigate, useApp } from "../router";
import type { Deployment, TaskState } from "../types";
import { Chip, ErrorNote, fmtNum, fmtTime, fmtUsd, isActiveState, label, relTime, statusTone, taskTone, toast } from "../ui";

const OPEN_LIMIT = 1000;
const CLOSED = new Set(["done", "cancelled"]);

function greeting(): string {
  const h = new Date().getHours();
  return h < 5 ? "Working late" : h < 12 ? "Good morning" : h < 17 ? "Good afternoon" : "Good evening";
}

export default function Overview() {
  const { projects, alerts, link, project: scope } = useApp();
  const status = link.state === "online" ? link.status : null;

  const open = useQuery(() => api.taskList({ open_only: true, limit: OPEN_LIMIT }), []);
  const deploys = useQuery(() => api.deployList(), []);
  const approvals = useQuery(() => api.approvalList("pending"), []);
  const usage = useQuery(() => api.usageSummary(7), []);

  useTpdEvents((ev) => {
    switch (ev.event) {
      case "task.updated": {
        const t = ev.data;
        open.set((prev) => {
          if (!prev) return prev;
          const rest = prev.filter((x) => x.id !== t.id);
          return CLOSED.has(t.state) ? rest : [t, ...rest];
        });
        break;
      }
      case "approval.created":
        approvals.set((prev) => (prev && !prev.some((a) => a.id === ev.data.id) ? [ev.data, ...prev] : prev));
        break;
      case "approval.decided":
        approvals.set((prev) => prev?.filter((a) => a.id !== ev.data.id));
        break;
      case "run.updated":
        if (ev.data.state !== "running" && ev.data.state !== "starting") usage.reload();
        break;
    }
  }, ["task.updated", "approval.created", "approval.decided", "run.updated"]);

  const inScope = <T extends { project?: string }>(xs: T[]) => (scope ? xs.filter((x) => x.project === scope) : xs);
  const openTasks = inScope(open.data ?? []);
  const blocked = openTasks.filter((t) => t.state === "blocked" || t.state === "escalated");
  // Running first, then anything waiting on you, then the rest by recency.
  const rank = (s: TaskState) => (isActiveState(s) ? 0 : ["blocked", "escalated", "awaiting_approval", "pr_open"].includes(s) ? 1 : 2);
  const work = [...openTasks].sort((a, b) => rank(a.state) - rank(b.state) || b.updated_at.localeCompare(a.updated_at));
  const pending = [...inScope(approvals.data ?? [])].sort((a, b) => Number(b.urgent) - Number(a.urgent));

  const byProject = useMemo(() => {
    const m = new Map<string, { open: number; blocked: number; active: number }>();
    for (const t of open.data ?? []) {
      const e = m.get(t.project) ?? { open: 0, blocked: 0, active: 0 };
      e.open++;
      if (t.state === "blocked" || t.state === "escalated") e.blocked++;
      if (isActiveState(t.state)) e.active++;
      m.set(t.project, e);
    }
    return m;
  }, [open.data]);

  const lastDeploy = useMemo(() => {
    const m = new Map<string, Deployment>();
    for (const d of deploys.data ?? []) {
      const prev = m.get(d.project);
      if (!prev || d.started_at > prev.started_at) m.set(d.project, d);
    }
    return m;
  }, [deploys.data]);

  const u = usage.data;

  return (
    <div className="home">
      <h1 className="greeting">
        <span className="logo">
          <Logo size={30} />
        </span>
        {greeting()}
      </h1>

      <Composer disabled={link.state !== "online"} />
      <Suggestions />

      {pending.length > 0 && (
        <section>
          <div className="section-h">
            <h2>
              Needs you <span className="count">{pending.length}</span>
            </h2>
            <a href="#/approvals" className="small">
              All approvals
            </a>
          </div>
          <div className="stack">
            {pending.slice(0, 3).map((a) => (
              <ApprovalCard key={a.id} approval={a} compact onDecided={(d) => approvals.set((prev) => prev?.filter((x) => x.id !== d.id))} />
            ))}
          </div>
        </section>
      )}

      {work.length > 0 && (
        <section>
          <div className="section-h">
            <h2>
              Open work <span className="count">{work.length}</span>
            </h2>
            <a href="#/tasks" className="small">
              All tasks
            </a>
          </div>
          <div className="rows">
            {work.slice(0, 6).map((t) => (
              <a key={t.id} className="rowlink" href={href({ view: "tasks", id: t.id })}>
                <span className={`dot ${isActiveState(t.state) ? "live" : taskTone(t.state)}`} />
                <span className="grow">
                  <span className="title">{t.title}</span>
                  <span className="sub">
                    {t.project} · {label(t.state)}
                    {t.blocked_reason ? ` · ${t.blocked_reason}` : ""} · {relTime(t.updated_at)}
                  </span>
                </span>
                <Icon name="chevronRight" className="go" />
              </a>
            ))}
          </div>
        </section>
      )}

      <section>
        <div className="section-h">
          <h2>Last 7 days</h2>
          <a href="#/usage" className="small">
            Usage & cost
          </a>
        </div>
        <ErrorNote error={usage.error} onRetry={usage.reload} />
        <div className="stats">
          <a className="stat" href="#/tasks">
            <span className="stat-label">Open tasks</span>
            <span className="stat-value">{open.data ? fmtNum(openTasks.length) : "–"}</span>
            <span className="stat-sub">{blocked.length ? `${blocked.length} blocked` : `${status?.active_runs ?? 0} running`}</span>
          </a>
          <a className="stat" href="#/tasks">
            <span className="stat-label">Tasks done</span>
            <span className="stat-value">{u ? fmtNum(u.tasks_done) : "–"}</span>
            <span className="stat-sub">{u ? `${u.tasks_blocked} blocked` : " "}</span>
          </a>
          <a className="stat" href="#/usage">
            <span className="stat-label">Agent runs</span>
            <span className="stat-value">{u ? fmtNum(u.runs) : "–"}</span>
            <span className="stat-sub">{u ? `${u.runs_failed} failed` : " "}</span>
          </a>
          <a className="stat" href="#/usage">
            <span className="stat-label">Reported cost</span>
            <span className="stat-value">{u ? fmtUsd(u.reported_cost_usd) : "–"}</span>
            <span className="stat-sub">list-price estimate</span>
          </a>
        </div>
      </section>

      <section>
        <div className="section-h">
          <h2>
            Projects <span className="count">{projects.length}</span>
          </h2>
          <a href="#/projects" className="small">
            Manage
          </a>
        </div>
        {projects.length === 0 ? (
          <div className="rows">
            <a className="rowlink" href="#/projects">
              <Icon name="plus" />
              <span className="grow">
                <span className="title">Add your first project</span>
                <span className="sub">Register a local git checkout so the factory can work on it.</span>
              </span>
            </a>
          </div>
        ) : (
          <div className="health-grid">
            {projects.map((p) => {
              const c = byProject.get(p.slug) ?? { open: 0, blocked: 0, active: 0 };
              const d = lastDeploy.get(p.slug);
              return (
                <button
                  key={p.slug}
                  className={`health ${c.blocked ? "has-blocked" : ""}`}
                  onClick={() => navigate({ view: "projects", id: p.slug })}
                  aria-label={`${p.slug}: ${c.open} open, ${c.blocked} blocked`}
                >
                  <span className="health-name">
                    <Icon name="folder" size={15} className="muted" />
                    {p.slug}
                    {c.active > 0 && <span className="dot live" title={`${c.active} running`} />}
                  </span>
                  <span className="health-stats">
                    <span>
                      <b>{c.open}</b> open
                    </span>
                    <span className={c.blocked ? "bad-text" : ""}>
                      <b>{c.blocked}</b> blocked
                    </span>
                    {p.jira_key && <span>Jira {p.jira_key}</span>}
                  </span>
                  <span className="health-deploy">
                    {d ? (
                      <>
                        <Chip tone={statusTone(d.state)}>{d.state}</Chip> {d.environment} · {relTime(d.started_at)}
                      </>
                    ) : (
                      "No deploys yet"
                    )}
                  </span>
                </button>
              );
            })}
          </div>
        )}
      </section>

      {alerts.length > 0 && (
        <section>
          <div className="section-h">
            <h2>
              Alerts <span className="count">{alerts.length}</span>
            </h2>
          </div>
          <div className="card">
            <ul className="alerts">
              {alerts.slice(0, 8).map((e) => (
                <li key={e.id} className={`alert ${e.alert.level}`}>
                  <Chip tone={statusTone(e.alert.level)}>{e.alert.level}</Chip>
                  <div style={{ minWidth: 0 }}>
                    <b>{e.alert.title}</b> <time className="muted small">{fmtTime(e.ts)}</time>
                    <p className="muted">{e.alert.body}</p>
                    {e.alert.task_id && <a href={href({ view: "tasks", id: e.alert.task_id })}>{e.alert.task_id}</a>}
                  </div>
                </li>
              ))}
            </ul>
          </div>
        </section>
      )}
    </div>
  );
}

// ------------------------------------------------------------ composer

function Composer({ disabled }: { disabled: boolean }) {
  const { projects, project: scope } = useApp();
  const [text, setText] = useState("");
  const [project, setProject] = useState(scope);
  const [now, setNow] = useState(true);
  const act = useAction();
  const box = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    if (!project && projects.length > 0) setProject(scope || projects[0].slug);
  }, [projects, scope, project]);

  useEffect(() => {
    const focus = () => box.current?.focus();
    window.addEventListener("tp:new-task", focus);
    return () => window.removeEventListener("tp:new-task", focus);
  }, []);

  // Grow with the text, up to the CSS max-height.
  useEffect(() => {
    const el = box.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
  }, [text]);

  const noProjects = projects.length === 0;
  const lines = text.trim().split("\n");
  const title = lines[0].trim().slice(0, 140);
  const canSend = !disabled && !noProjects && !!project && title.length > 0 && !act.busy;

  const submit = async (e?: FormEvent) => {
    e?.preventDefault();
    if (!canSend) return;
    const body = lines.length > 1 || lines[0].trim().length > 140 ? text.trim() : undefined;
    const t = await act.run(() => api.taskCreate({ project, title, body, approve_now: now }));
    if (t) {
      setText("");
      toast(now ? `${t.id} queued — the factory will start it.` : `${t.id} added to the daily plan.`);
      navigate({ view: "tasks", id: t.id });
    }
  };

  return (
    <form className="composer" onSubmit={submit}>
      <textarea
        ref={box}
        rows={2}
        autoFocus
        placeholder={
          noProjects ? "Add a project first, then describe work for the factory…" : "Describe a task for the factory… (first line is the title)"
        }
        value={text}
        disabled={disabled || noProjects}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
            e.preventDefault();
            void submit();
          }
        }}
        aria-label="New task"
      />
      <ErrorNote error={act.error} />
      <div className="composer-bar">
        {noProjects ? (
          <a className="btn small" href="#/projects">
            <Icon name="plus" size={14} /> Add project
          </a>
        ) : (
          <select value={project} onChange={(e) => setProject(e.target.value)} aria-label="Project">
            {projects.map((p) => (
              <option key={p.slug} value={p.slug}>
                {p.slug}
              </option>
            ))}
          </select>
        )}
        <label className="check" title="Start now instead of waiting for the daily plan (still policy-checked)">
          <input type="checkbox" checked={now} onChange={(e) => setNow(e.target.checked)} />
          Start now
        </label>
        <button type="submit" className="send" disabled={!canSend} aria-label="Create task" title="Create task (↵)">
          {act.busy ? <span className="spinner" style={{ borderTopColor: "#fff" }} /> : <Icon name="arrowUp" />}
        </button>
      </div>
    </form>
  );
}

function Suggestions() {
  const act = useAction();
  const run = async (what: "plan" | "intake") => {
    if (what === "plan") {
      const p = await act.run(() => api.planGenerate());
      if (p) navigate({ view: "plan" });
    } else {
      const r = await act.run(() => api.intakeReconcile());
      if (r) toast(`GitHub checked: ${r.signals_new} new signals, ${r.tasks_created} tasks created.`);
    }
  };
  useEffect(() => {
    if (act.error) toast(act.error, "bad");
  }, [act.error]);
  return (
    <div className="suggestions">
      <button className="suggestion" disabled={act.busy} onClick={() => void run("plan")}>
        <Icon name="calendar" size={14} /> Draft today's plan
      </button>
      <button className="suggestion" disabled={act.busy} onClick={() => void run("intake")}>
        <Icon name="refresh" size={14} /> Check GitHub for work
      </button>
      <a className="suggestion" href="#/chat" style={{ textDecoration: "none" }}>
        <Icon name="chat" size={14} /> Ask the manager
      </a>
      <a className="suggestion" href="#/projects" style={{ textDecoration: "none" }}>
        <Icon name="shield" size={14} /> Scan dependencies
      </a>
    </div>
  );
}
