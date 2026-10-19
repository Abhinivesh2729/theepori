// Coding Workspace: one task, its runs (live), diff, checks, findings,
// approvals and deployments, with run controls.

import { useCallback, useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import { api, useAction, useQuery, useTpdEvents } from "../api";
import { ApprovalCard } from "../ApprovalCard";
import type { Run, RunEvent, TaskDetail as Detail, TaskDiff } from "../types";
import { VirtualList, type VirtualListHandle } from "../VirtualList";
import { DeployTable } from "../widgets";
import {
  A,
  Chip,
  Confirm,
  Empty,
  ErrorNote,
  Ext,
  Field,
  fmtMs,
  fmtNum,
  fmtTime,
  fmtUsd,
  JsonBlock,
  label,
  Loading,
  Modal,
  RiskChip,
  runTone,
  StateChip,
  statusTone,
  Tabs,
} from "../ui";

type Tab = "activity" | "diff" | "checks" | "findings" | "approvals" | "deploys" | "brief";
const ACTIVE_RUN = new Set(["starting", "running"]);
const FINAL_TASK = new Set(["done", "cancelled", "merged"]);

export default function TaskDetail({ id }: { id: string }) {
  const q = useQuery(() => api.taskGet(id), [id]);
  const [tab, setTab] = useState<Tab>("activity");
  const [runId, setRunId] = useState<string | null>(null);
  const [dialog, setDialog] = useState<null | "stop" | "cancel" | "message">(null);
  const [notice, setNotice] = useState<string | null>(null);
  const act = useAction();

  useTpdEvents((ev) => {
    if (ev.event === "task.updated" && ev.data.id === id) {
      q.set((d) => (d ? { ...d, task: ev.data } : d));
    } else if (ev.event === "run.updated" && ev.data.task_id === id) {
      const r = ev.data;
      q.set((d) => {
        if (!d) return d;
        const runs = d.runs.some((x) => x.id === r.id) ? d.runs.map((x) => (x.id === r.id ? r : x)) : [...d.runs, r];
        return { ...d, runs };
      });
    } else if ((ev.event === "approval.created" || ev.event === "approval.decided") && ev.data.task_id === id) {
      q.reload();
    }
  }, ["task.updated", "run.updated", "approval.created", "approval.decided"]);

  const d = q.data;
  const runs = useMemo(() => [...(d?.runs ?? [])].sort((a, b) => a.started_at.localeCompare(b.started_at)), [d?.runs]);
  const activeRun = runs.find((r) => ACTIVE_RUN.has(r.state));
  const selectedRun = runs.find((r) => r.id === runId) ?? activeRun ?? runs.at(-1);

  if (q.error && !d) return <ErrorNote error={q.error} onRetry={q.reload} />;
  if (!d) return <Loading what="Loading task" />;
  const t = d.task;

  const doRetry = async () => {
    const r = await act.run(() => api.taskRetry(t.id));
    if (r) setNotice("Retry queued.");
  };
  const doTerminal = async () => {
    const r = await act.run(() => api.taskOpenTerminal(t.id));
    if (r) setNotice(r.opened ? "Opened in Terminal." : "Terminal could not be opened; check that the worktree exists.");
  };

  const pendingApprovals = d.approvals.filter((a) => a.state === "pending").length;
  const openFindings = d.findings.filter((f) => f.status === "open").length;
  const failedChecks = d.verifications.filter((v) => v.status === "failed" || v.status === "blocked").length;

  return (
    <div className="page fill workspace">
      <header className="ws-head">
        <div className="ws-title">
          <A to={{ view: "tasks" }} className="back">
            ← Tasks
          </A>
          <h1>{t.title}</h1>
        </div>
        <div className="ws-meta">
          <span className="mono">{t.id}</span>
          <span>{t.project}</span>
          <StateChip state={t.state} />
          <RiskChip risk={t.risk} />
          <Chip>{t.category}</Chip>
          <Chip>P{t.priority}</Chip>
          <span className="muted">attempt {t.attempts}</span>
          {t.jira_key && <Chip tone="accent">{t.jira_key}</Chip>}
          {t.pr_url && <Ext url={t.pr_url}>Pull request</Ext>}
          {t.source_url && <Ext url={t.source_url}>Source ({t.source})</Ext>}
          {d.branch && <span className="mono muted">{d.branch}</span>}
        </div>
        {t.blocked_reason && (
          <div className="note warn" role="status">
            Blocked: {t.blocked_reason}
          </div>
        )}
        <div className="ws-controls" role="toolbar" aria-label="Task controls">
          <button className="btn danger" disabled={!activeRun || act.busy} onClick={() => setDialog("stop")}>
            Stop run
          </button>
          <button className="btn" disabled={!!activeRun || act.busy || t.state === "done"} onClick={() => void doRetry()}>
            Retry
          </button>
          <button className="btn" disabled={FINAL_TASK.has(t.state) || act.busy} onClick={() => setDialog("cancel")}>
            Cancel task
          </button>
          <button className="btn primary" disabled={act.busy} onClick={() => setDialog("message")}>
            Send message to agent
          </button>
          <button className="btn" disabled={act.busy} onClick={() => void doTerminal()}>
            Open in Terminal
          </button>
          {notice && (
            <span className="muted small" role="status">
              {notice}
            </span>
          )}
        </div>
        <ErrorNote error={act.error} />
      </header>

      <Tabs
        ariaLabel="Task sections"
        value={tab}
        onChange={setTab}
        tabs={[
          { key: "activity", label: activeRun ? "Activity ●" : "Activity" },
          { key: "diff", label: "Diff" },
          { key: "checks", label: `Verification${failedChecks ? ` (${failedChecks} failing)` : ""}` },
          { key: "findings", label: `Review findings${openFindings ? ` (${openFindings})` : ""}` },
          { key: "approvals", label: `Approvals${pendingApprovals ? ` (${pendingApprovals})` : ""}` },
          { key: "deploys", label: "Deployments" },
          { key: "brief", label: "Brief & runs" },
        ]}
      />
      <section className="ws-panel" role="tabpanel" aria-label={tab}>
        {tab === "activity" &&
          (selectedRun ? (
            <Activity runs={runs} run={selectedRun} onSelectRun={setRunId} />
          ) : (
            <Empty>No runs yet. The task starts once it is approved and assigned.</Empty>
          ))}
        {tab === "diff" && <DiffView taskId={t.id} />}
        {tab === "checks" && <Checks d={d} />}
        {tab === "findings" && <Findings d={d} />}
        {tab === "approvals" &&
          (d.approvals.length === 0 ? (
            <Empty>No approvals for this task.</Empty>
          ) : (
            <div className="stack">
              {d.approvals.map((a) => (
                <ApprovalCard key={a.id} approval={a} compact={a.state !== "pending"} onDecided={() => q.reload()} />
              ))}
            </div>
          ))}
        {tab === "deploys" && <Deploys d={d} />}
        {tab === "brief" && <Brief d={d} runs={runs} onOpenRun={(r) => (setRunId(r.id), setTab("activity"))} />}
      </section>

      {dialog === "stop" && activeRun && (
        <StopDialog run={activeRun} onClose={() => setDialog(null)} onDone={(r) => (setDialog(null), setNotice(`Run ${r.id} ${r.state}.`))} />
      )}
      {dialog === "cancel" && (
        <CancelDialog taskId={t.id} onClose={() => setDialog(null)} onDone={() => (setDialog(null), q.reload())} />
      )}
      {dialog === "message" && (
        <MessageDialog
          taskId={t.id}
          onClose={() => setDialog(null)}
          onDone={(r) => {
            setDialog(null);
            q.set((x) => (x ? { ...x, runs: [...x.runs.filter((y) => y.id !== r.id), r] } : x));
            setRunId(r.id);
            setTab("activity");
          }}
        />
      )}
    </div>
  );
}

// ------------------------------------------------------------ dialogs

function StopDialog({ run, onClose, onDone }: { run: Run; onClose: () => void; onDone: (r: Run) => void }) {
  const act = useAction();
  return (
    <Confirm
      title="Stop this run?"
      body={
        <p>
          Interrupts the {run.kind} run <span className="mono">{run.id}</span>. The session is kept, so you can continue it with a message
          or retry later.
        </p>
      }
      confirmLabel="Stop run"
      danger
      busy={act.busy}
      error={act.error}
      onClose={onClose}
      onConfirm={() => void act.run(() => api.runStop(run.id)).then((r) => r && onDone(r))}
    />
  );
}

function CancelDialog({ taskId, onClose, onDone }: { taskId: string; onClose: () => void; onDone: () => void }) {
  const act = useAction();
  const [reason, setReason] = useState("");
  return (
    <Modal title="Cancel task?" onClose={onClose}>
      <p className="modal-body">Stops any run, releases the worktree lock and marks the task cancelled. This is audited.</p>
      <Field label="Reason (optional)">
        <input value={reason} onChange={(e) => setReason(e.target.value)} />
      </Field>
      <ErrorNote error={act.error} />
      <div className="modal-foot">
        <button className="btn" onClick={onClose}>
          Keep task
        </button>
        <button
          className="btn danger"
          disabled={act.busy}
          onClick={() => void act.run(() => api.taskCancel(taskId, reason.trim() || undefined)).then((r) => r && onDone())}
        >
          Cancel task
        </button>
      </div>
    </Modal>
  );
}

function MessageDialog({ taskId, onClose, onDone }: { taskId: string; onClose: () => void; onDone: (r: Run) => void }) {
  const act = useAction();
  const [text, setText] = useState("");
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!text.trim()) return;
    void act.run(() => api.taskMessage(taskId, text.trim())).then((r) => r && onDone(r));
  };
  return (
    <Modal title="Send message to agent" onClose={onClose} wide>
      <form onSubmit={submit} className="form">
        <p className="muted">Resumes the task's session with your message as the next turn. The run is audited as an intervention.</p>
        <Field label="Message">
          <textarea
            rows={6}
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) submit(e);
            }}
          />
        </Field>
        <ErrorNote error={act.error} />
        <div className="modal-foot">
          <span className="muted small">⌘↵ to send</span>
          <button type="button" className="btn" onClick={onClose}>
            Cancel
          </button>
          <button type="submit" className="btn primary" disabled={!text.trim() || act.busy}>
            {act.busy ? "Sending…" : "Send"}
          </button>
        </div>
      </form>
    </Modal>
  );
}

// ------------------------------------------------------------ activity

const PAGE = 5000;
const MAX_EVENTS = 300_000;
const ROW_H = 24;
const KINDS = ["system", "text", "tool_use", "tool_result", "denied", "result", "stderr", "tp"];

function Activity({ runs, run, onSelectRun }: { runs: Run[]; run: Run; onSelectRun: (id: string) => void }) {
  // Events live in a ref; `version` triggers renders. Avoids copying 100k-row
  // arrays on every live event.
  const events = useRef<RunEvent[]>([]);
  const lastSeq = useRef(-1);
  const pending = useRef<RunEvent[]>([]);
  const paging = useRef(true);
  const frame = useRef(0);
  const [version, setVersion] = useState(0);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [follow, setFollow] = useState(true);
  const [kind, setKind] = useState("");
  const [needle, setNeedle] = useState("");
  const [selected, setSelected] = useState<number | null>(null);
  const list = useRef<VirtualListHandle>(null);

  const append = useCallback((batch: RunEvent[]) => {
    let added = false;
    for (const e of batch) {
      if (e.seq > lastSeq.current) {
        events.current.push(e);
        lastSeq.current = e.seq;
        added = true;
      }
    }
    if (events.current.length > MAX_EVENTS) events.current.splice(0, events.current.length - MAX_EVENTS);
    return added;
  }, []);

  const flush = useCallback(() => {
    frame.current = 0;
    if (paging.current || pending.current.length === 0) return;
    const batch = pending.current.sort((a, b) => a.seq - b.seq);
    pending.current = [];
    if (append(batch)) setVersion((v) => v + 1);
  }, [append]);

  useEffect(() => {
    let live = true;
    events.current = [];
    lastSeq.current = -1;
    pending.current = [];
    paging.current = true;
    setSelected(null);
    setLoading(true);
    setError(null);
    setVersion((v) => v + 1);
    (async () => {
      try {
        for (;;) {
          const after = lastSeq.current >= 0 ? lastSeq.current : undefined;
          const page = await api.runEvents(run.id, after, PAGE);
          if (!live) return;
          append(page);
          setVersion((v) => v + 1);
          if (page.length < PAGE || events.current.length >= MAX_EVENTS) break;
        }
      } catch (e) {
        if (live) setError(e instanceof Error ? e.message : String(e));
      } finally {
        if (live) {
          paging.current = false;
          setLoading(false);
          flush();
        }
      }
    })();
    return () => {
      live = false;
      cancelAnimationFrame(frame.current);
      frame.current = 0;
    };
  }, [run.id, append, flush]);

  useTpdEvents((ev) => {
    if (ev.event !== "run.event" || ev.data.run_id !== run.id) return;
    pending.current.push(ev.data);
    if (!frame.current) frame.current = requestAnimationFrame(flush);
  }, ["run.event"]);

  const filtered = useMemo(() => {
    const q = needle.trim().toLowerCase();
    if (!kind && !q) return null;
    const out: number[] = [];
    const ev = events.current;
    for (let i = 0; i < ev.length; i++) {
      const e = ev[i];
      if (kind && e.kind !== kind) continue;
      if (q && !e.text.toLowerCase().includes(q)) continue;
      out.push(i);
    }
    return out;
    // `version` stands in for the mutable events array.
  }, [version, kind, needle]);

  const count = filtered ? filtered.length : events.current.length;
  const at = (i: number) => events.current[filtered ? filtered[i] : i];
  const sel = selected != null ? events.current[selected] : undefined;

  return (
    <div className="activity">
      <div className="filters">
        <select
          value={run.id}
          onChange={(e) => onSelectRun(e.target.value)}
          aria-label="Run"
        >
          {runs.map((r) => (
            <option key={r.id} value={r.id}>
              {r.kind} · {r.state} · {fmtTime(r.started_at)}
            </option>
          ))}
        </select>
        <Chip tone={runTone(run.state)}>{run.state}</Chip>
        <span className="muted small">
          {run.model ?? run.runtime}
          {run.num_turns != null && ` · ${run.num_turns} turns`}
          {run.cost_usd != null && ` · ${fmtUsd(run.cost_usd)} reported`}
        </span>
        <select value={kind} onChange={(e) => setKind(e.target.value)} aria-label="Event kind">
          <option value="">All kinds</option>
          {KINDS.map((k) => (
            <option key={k} value={k}>
              {label(k)}
            </option>
          ))}
        </select>
        <input type="search" placeholder="Filter events" value={needle} onChange={(e) => setNeedle(e.target.value)} aria-label="Filter events" />
        <label className="check">
          <input
            type="checkbox"
            checked={follow}
            onChange={(e) => {
              setFollow(e.target.checked);
              if (e.target.checked) list.current?.scrollToEnd();
            }}
          />
          Autoscroll
        </label>
        <span className="muted small" aria-live="polite">
          {fmtNum(count)} events{loading ? " · loading…" : ""}
        </span>
      </div>
      <ErrorNote error={error} />
      {run.error && <div className="note bad">{run.error}</div>}
      <VirtualList
        ref={list}
        className="log"
        count={count}
        rowHeight={ROW_H}
        stickToEnd={follow}
        onAtEndChange={(end) => !end && setFollow(false)}
        ariaLabel="Run events"
        renderRow={(i) => {
          const e = at(i);
          if (!e) return null;
          const idx = filtered ? filtered[i] : i;
          return (
            <button
              className={`log-row k-${e.kind} ${idx === selected ? "sel" : ""}`}
              onClick={() => setSelected(idx === selected ? null : idx)}
              title={e.text}
            >
              <span className="log-seq">{e.seq}</span>
              <span className="log-ts">{e.ts.slice(11, 19)}</span>
              <span className="log-kind">{e.kind}</span>
              <span className="log-text">{e.text}</span>
            </button>
          );
        }}
      />
      {sel && (
        <div className="log-detail" aria-label="Event detail">
          <div className="log-detail-head">
            <b>
              #{sel.seq} {sel.kind}
            </b>
            <span className="muted">{sel.ts}</span>
            <button className="btn small ghost" onClick={() => setSelected(null)}>
              Close
            </button>
          </div>
          <p className="pre">{sel.text}</p>
          <JsonBlock value={sel.data} />
        </div>
      )}
      {run.summary && !sel && (
        <p className="run-summary">
          <b>Summary (claimed by agent):</b> {run.summary}
        </p>
      )}
    </div>
  );
}

// ------------------------------------------------------------ diff

const DIFF_ROW_H = 18;

interface ParsedDiff {
  lines: string[];
  oldNo: Int32Array;
  newNo: Int32Array;
  fileAt: Map<string, number>;
}

function parseDiff(d: TaskDiff): ParsedDiff {
  const lines = d.diff.split("\n");
  if (lines.length && lines[lines.length - 1] === "") lines.pop();
  const oldNo = new Int32Array(lines.length);
  const newNo = new Int32Array(lines.length);
  const fileAt = new Map<string, number>();
  let o = 0;
  let n = 0;
  for (let i = 0; i < lines.length; i++) {
    const l = lines[i];
    if (l.startsWith("diff --git ")) {
      const m = / b\/(.+)$/.exec(l);
      if (m) fileAt.set(m[1], i);
      continue;
    }
    if (l.startsWith("@@")) {
      const m = /^@@ -(\d+)(?:,\d+)? \+(\d+)/.exec(l);
      if (m) {
        o = Number(m[1]);
        n = Number(m[2]);
      }
      continue;
    }
    if (l.startsWith("+++") || l.startsWith("---")) continue;
    const c = l[0];
    if (c === "+") newNo[i] = n++;
    else if (c === "-") oldNo[i] = o++;
    else if (c === " ") {
      oldNo[i] = o++;
      newNo[i] = n++;
    }
  }
  return { lines, oldNo, newNo, fileAt };
}

function lineClass(l: string): string {
  if (l.startsWith("diff --git")) return "d-file";
  if (l.startsWith("+++") || l.startsWith("---") || l.startsWith("index ")) return "d-meta";
  if (l.startsWith("@@")) return "d-hunk";
  if (l[0] === "+") return "d-add";
  if (l[0] === "-") return "d-del";
  return "d-ctx";
}

function DiffView({ taskId }: { taskId: string }) {
  const q = useQuery(() => api.taskDiff(taskId), [taskId]);
  const parsed = useMemo(() => (q.data ? parseDiff(q.data) : null), [q.data]);
  const list = useRef<VirtualListHandle>(null);
  const [file, setFile] = useState<string | null>(null);

  if (q.error) return <ErrorNote error={q.error} onRetry={q.reload} />;
  if (!q.data || !parsed) return <Loading what="Loading diff" />;
  const d = q.data;
  if (d.files.length === 0) return <Empty>No changes against {d.base}.</Empty>;
  const adds = d.files.reduce((s, f) => s + f.additions, 0);
  const dels = d.files.reduce((s, f) => s + f.deletions, 0);

  return (
    <div className="diff">
      <aside className="diff-files" aria-label="Changed files">
        <p className="muted small">
          {d.files.length} files · <span className="add-text">+{adds}</span> <span className="del-text">−{dels}</span> vs{" "}
          <span className="mono">{d.base}</span>
        </p>
        <ul>
          {d.files.map((f) => (
            <li key={f.path}>
              <button
                className={file === f.path ? "sel" : ""}
                onClick={() => {
                  setFile(f.path);
                  const i = parsed.fileAt.get(f.path);
                  if (i != null) list.current?.scrollToIndex(i);
                }}
                title={f.path}
              >
                <span className="diff-path">{f.path}</span>
                <span className="add-text">+{f.additions}</span>
                <span className="del-text">−{f.deletions}</span>
              </button>
            </li>
          ))}
        </ul>
        <button className="btn small" onClick={q.reload}>
          Refresh
        </button>
      </aside>
      <div className="diff-main">
        {d.truncated && (
          <div className="note warn">Diff truncated by tpd; open the worktree in Terminal for the full change.</div>
        )}
        <VirtualList
          ref={list}
          className="diff-lines"
          count={parsed.lines.length}
          rowHeight={DIFF_ROW_H}
          overscan={30}
          ariaLabel="Unified diff"
          renderRow={(i) => {
            const l = parsed.lines[i];
            return (
              <div className={`d-line ${lineClass(l)}`}>
                <span className="d-no">{parsed.oldNo[i] || ""}</span>
                <span className="d-no">{parsed.newNo[i] || ""}</span>
                <span className="d-text">{l}</span>
              </div>
            );
          }}
        />
      </div>
    </div>
  );
}

// ------------------------------------------------------------ other tabs

function Checks({ d }: { d: Detail }) {
  if (d.verifications.length === 0) return <Empty>No verification results yet. Missing evidence counts as not passed.</Empty>;
  const rows = [...d.verifications].sort((a, b) => b.attempt - a.attempt || a.name.localeCompare(b.name));
  return (
    <table className="dense">
      <thead>
        <tr>
          <th>Attempt</th>
          <th>Check</th>
          <th>Kind</th>
          <th>Status</th>
          <th>Duration</th>
          <th>Summary</th>
          <th>Evidence</th>
        </tr>
      </thead>
      <tbody>
        {rows.map((v) => (
          <tr key={v.id}>
            <td>{v.attempt}</td>
            <td>{v.name}</td>
            <td className="muted">{v.kind}</td>
            <td>
              <Chip tone={statusTone(v.status)}>{label(v.status)}</Chip>
            </td>
            <td className="muted">{fmtMs(v.duration_ms)}</td>
            <td>{v.summary}</td>
            <td className="mono small">{v.evidence_path ?? <span className="muted">none</span>}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function Findings({ d }: { d: Detail }) {
  if (d.findings.length === 0) return <Empty>No reviewer findings.</Empty>;
  const order = { blocking: 0, major: 1, minor: 2, info: 3 } as Record<string, number>;
  const rows = [...d.findings].sort((a, b) => (order[a.severity] ?? 9) - (order[b.severity] ?? 9));
  return (
    <>
      <p className="muted small">Findings come from a separate reviewer run. They never count as proof that the change works.</p>
      <table className="dense">
        <thead>
          <tr>
            <th>Severity</th>
            <th>Location</th>
            <th>Finding</th>
            <th>Status</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((f) => (
            <tr key={f.id} className={f.status === "resolved" ? "dim" : ""}>
              <td>
                <Chip tone={statusTone(f.severity)}>{f.severity}</Chip>
              </td>
              <td className="mono small">{f.file ? `${f.file}${f.line != null ? `:${f.line}` : ""}` : "–"}</td>
              <td>{f.message}</td>
              <td>
                <Chip tone={statusTone(f.status)}>{f.status}</Chip>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </>
  );
}

function Deploys({ d }: { d: Detail }) {
  if (d.deployments.length === 0) return <Empty>Not deployed.</Empty>;
  return <DeployTable rows={d.deployments} />;
}

function Brief({ d, runs, onOpenRun }: { d: Detail; runs: Run[]; onOpenRun: (r: Run) => void }) {
  return (
    <div className="brief">
      <section>
        <h3>Description</h3>
        {d.body ? <p className="pre">{d.body}</p> : <p className="muted">No description.</p>}
        <h3>Acceptance criteria</h3>
        {d.acceptance.length === 0 ? (
          <p className="muted">Not defined yet.</p>
        ) : (
          <ul>
            {d.acceptance.map((a, i) => (
              <li key={i}>{a}</li>
            ))}
          </ul>
        )}
        <dl className="kv">
          <dt>Branch</dt>
          <dd className="mono">{d.branch ?? "–"}</dd>
          <dt>Worktree</dt>
          <dd className="mono">{d.worktree ?? "–"}</dd>
          <dt>Source</dt>
          <dd>{d.task.source}</dd>
          <dt>Created</dt>
          <dd>{fmtTime(d.task.created_at)}</dd>
        </dl>
      </section>
      <section>
        <h3>Runs</h3>
        {runs.length === 0 ? (
          <p className="muted">No runs.</p>
        ) : (
          <table className="dense">
            <thead>
              <tr>
                <th>Kind</th>
                <th>State</th>
                <th>Model</th>
                <th>Started</th>
                <th>Turns</th>
                <th>Tokens in/out</th>
                <th>Reported cost</th>
              </tr>
            </thead>
            <tbody>
              {runs.map((r) => (
                <tr key={r.id}>
                  <td>
                    <button className="linkish" onClick={() => onOpenRun(r)}>
                      {r.kind}
                    </button>
                  </td>
                  <td>
                    <Chip tone={runTone(r.state)}>{r.state}</Chip>
                  </td>
                  <td className="muted">{r.model ?? r.runtime}</td>
                  <td className="muted">{fmtTime(r.started_at)}</td>
                  <td>{r.num_turns ?? "–"}</td>
                  <td>
                    {fmtNum(r.input_tokens)} / {fmtNum(r.output_tokens)}
                  </td>
                  <td>{fmtUsd(r.cost_usd)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>
    </div>
  );
}
