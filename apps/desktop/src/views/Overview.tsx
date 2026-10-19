// Mission Control: one screen for "is the factory healthy and what needs me".

import { useMemo, useState } from "react";
import { api, useQuery, useTpdEvents } from "../api";
import { ApprovalCard } from "../ApprovalCard";
import { navigate, useApp } from "../router";
import type { Deployment, Run, TaskSummary } from "../types";
import { AuditList, BudgetBar, Tile } from "../widgets";
import {
  A,
  Card,
  Chip,
  Empty,
  ErrorNote,
  fmtNum,
  fmtTime,
  fmtUsd,
  isActiveState,
  Loading,
  relTime,
  RiskChip,
  runTone,
  StateChip,
  statusTone,
} from "../ui";

const AUDIT_LIMIT = 20;
const OPEN_LIMIT = 1000;
const CLOSED = new Set(["done", "cancelled"]);

export default function Overview() {
  const { projects, alerts, link } = useApp();
  const status = link.state === "online" ? link.status : null;

  const open = useQuery(() => api.taskList({ open_only: true, limit: OPEN_LIMIT }), []);
  const deploys = useQuery(() => api.deployList(), []);
  const plan = useQuery(() => api.planCurrent(), []);
  const approvals = useQuery(() => api.approvalList("pending"), []);
  const usage = useQuery(() => api.usageSummary(7), []);
  const audit = useQuery(() => api.auditTail(AUDIT_LIMIT).then((a) => [...a].reverse()), []);
  const [runs, setRuns] = useState<Record<string, Run>>({});

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
      case "plan.updated":
        plan.set(ev.data);
        break;
      case "run.updated":
        setRuns((prev) => ({ ...prev, [ev.data.id]: ev.data }));
        break;
      case "audit.appended":
        audit.set((prev) => (prev ? [ev.data, ...prev].slice(0, AUDIT_LIMIT) : prev));
        break;
    }
  }, ["task.updated", "approval.created", "approval.decided", "plan.updated", "run.updated", "audit.appended"]);

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

  const activeTasks = useMemo(() => (open.data ?? []).filter((t) => isActiveState(t.state)), [open.data]);
  const liveRuns = Object.values(runs).filter((r) => r.state === "running" || r.state === "starting");
  const today = usage.data?.by_day.at(-1);
  const urgent = (approvals.data ?? []).filter((a) => a.urgent);

  return (
    <div className="grid overview">
      <Card title="Projects" count={projects.length} className="span-2">
        {projects.length === 0 ? (
          <Empty>
            No projects yet. <A to={{ view: "projects" }}>Add one</A> or run <code>tp project add &lt;path&gt; --jira KEY</code>.
          </Empty>
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
                  <span className="health-name">{p.slug}</span>
                  <span className="health-stats">
                    <span>
                      <b>{c.open}</b> open
                    </span>
                    <span className={c.blocked ? "bad-text" : ""}>
                      <b>{c.blocked}</b> blocked
                    </span>
                    <span>
                      <b>{c.active}</b> active
                    </span>
                  </span>
                  <span className="health-deploy">
                    {d ? (
                      <>
                        <Chip tone={statusTone(d.state)}>{d.state}</Chip> {d.environment} · {relTime(d.started_at)}
                      </>
                    ) : (
                      <span className="muted">no deploys</span>
                    )}
                  </span>
                </button>
              );
            })}
          </div>
        )}
        <ErrorNote error={open.error ?? deploys.error} />
      </Card>

      <Card title="Usage · 7 days" className="tiles-card">
        {usage.error ? (
          <ErrorNote error={usage.error} onRetry={usage.reload} />
        ) : !usage.data ? (
          <Loading />
        ) : (
          <>
            <div className="tiles">
              <Tile label="Runs" value={fmtNum(usage.data.runs)} sub={`${usage.data.runs_failed} failed`} />
              <Tile label="Reported cost" value={fmtUsd(usage.data.reported_cost_usd)} sub="list-price estimate" />
              <Tile label="Today" value={fmtUsd(today?.cost_usd ?? 0)} sub={`${today?.runs ?? 0} runs`} />
              <Tile label="Tasks done" value={fmtNum(usage.data.tasks_done)} sub={`${usage.data.tasks_blocked} blocked`} />
            </div>
            {usage.data.budgets.length > 0 && (
              <ul className="budget-bars">
                {usage.data.budgets.map((b) => (
                  <BudgetBar key={`${b.budget.scope}:${b.budget.period}`} spent={b.spent_usd} limit={b.budget.limit_usd} name={`${b.budget.scope} / ${b.budget.period}`} exceeded={b.exceeded} />
                ))}
              </ul>
            )}
            <A to={{ view: "usage" }} className="more">
              Usage & cost →
            </A>
          </>
        )}
      </Card>

      <Card
        title="Pending approvals"
        count={approvals.data?.length}
        className="span-2"
        actions={<A to={{ view: "approvals" }}>Approval Center →</A>}
      >
        <ErrorNote error={approvals.error} onRetry={approvals.reload} />
        {approvals.data && approvals.data.length === 0 && <Empty>Nothing waiting on you.</Empty>}
        {urgent.length > 0 && <p className="bad-text small">{urgent.length} urgent</p>}
        <div className="stack">
          {[...(approvals.data ?? [])]
            .sort((a, b) => Number(b.urgent) - Number(a.urgent))
            .slice(0, 4)
            .map((a) => (
              <ApprovalCard key={a.id} approval={a} compact onDecided={(d) => approvals.set((prev) => prev?.filter((x) => x.id !== d.id))} />
            ))}
        </div>
      </Card>

      <Card title="Today's plan" actions={<A to={{ view: "plan" }}>Daily Plan →</A>}>
        <ErrorNote error={plan.error} onRetry={plan.reload} />
        {plan.loading && !plan.data ? (
          <Loading />
        ) : !plan.data ? (
          <Empty>No plan yet today.</Empty>
        ) : (
          <>
            <p>
              <Chip tone={statusTone(plan.data.state)}>{plan.data.state}</Chip> {plan.data.date} · {plan.data.items.length} items
            </p>
            <p className="muted clamp-3">{plan.data.summary}</p>
            <ol className="mini-list">
              {plan.data.items.slice(0, 5).map((i) => (
                <li key={i.task.id}>
                  <A to={{ view: "tasks", id: i.task.id }}>{i.task.title}</A> <span className="muted">{i.task.project}</span>
                </li>
              ))}
            </ol>
          </>
        )}
      </Card>

      <Card title="Active work" count={status?.active_runs ?? activeTasks.length} className="span-2">
        {activeTasks.length === 0 && liveRuns.length === 0 ? (
          <Empty>No agents running.</Empty>
        ) : (
          <table className="dense">
            <thead>
              <tr>
                <th>Task</th>
                <th>Project</th>
                <th>State</th>
                <th>Risk</th>
                <th>Updated</th>
              </tr>
            </thead>
            <tbody>
              {activeTasks.slice(0, 12).map((t: TaskSummary) => (
                <tr key={t.id}>
                  <td>
                    <A to={{ view: "tasks", id: t.id }}>{t.title}</A>
                  </td>
                  <td>{t.project}</td>
                  <td>
                    <StateChip state={t.state} />
                  </td>
                  <td>
                    <RiskChip risk={t.risk} />
                  </td>
                  <td className="muted">{relTime(t.updated_at)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        {liveRuns.length > 0 && (
          <ul className="mini-list">
            {liveRuns.map((r) => (
              <li key={r.id}>
                <Chip tone={runTone(r.state)}>{r.kind}</Chip>{" "}
                {r.task_id ? <A to={{ view: "tasks", id: r.task_id }}>{r.task_id}</A> : r.id} <span className="muted">{r.model ?? r.runtime}</span>
              </li>
            ))}
          </ul>
        )}
      </Card>

      <Card title="Alerts" count={alerts.length}>
        {alerts.length === 0 ? (
          <Empty>No alerts since this window opened.</Empty>
        ) : (
          <ul className="alerts">
            {alerts.slice(0, 15).map((e) => (
              <li key={e.id} className={`alert ${e.alert.level}`}>
                <Chip tone={statusTone(e.alert.level)}>{e.alert.level}</Chip>
                <div>
                  <b>{e.alert.title}</b> <time className="muted">{fmtTime(e.ts)}</time>
                  <p>{e.alert.body}</p>
                  {e.alert.task_id && <A to={{ view: "tasks", id: e.alert.task_id }}>{e.alert.task_id}</A>}
                </div>
              </li>
            ))}
          </ul>
        )}
      </Card>

      <Card title="Recent audit" className="span-3" actions={<A to={{ view: "settings", id: "audit" }}>Audit log →</A>}>
        <ErrorNote error={audit.error} onRetry={audit.reload} />
        <AuditList records={audit.data ?? []} />
      </Card>
    </div>
  );
}
