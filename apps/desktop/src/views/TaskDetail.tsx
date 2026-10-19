// Coding Workspace: one task as a conversation with its agents (live), plus
// its diff, checks, review findings, approvals and deployments.

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type FormEvent, type ReactNode } from "react";
import { api, useAction, useQuery, useTpdEvents } from "../api";
import { ApprovalCard } from "../ApprovalCard";
import { Icon } from "../icons";
import type { Run, RunEvent, TaskDetail as Detail, TaskDiff } from "../types";
import { VirtualList, type VirtualListHandle } from "../VirtualList";
import { DeployTable } from "../widgets";
import {
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
  toast,
} from "../ui";

type Tab = "conversation" | "log" | "diff" | "checks" | "findings" | "approvals" | "deploys" | "brief";
const ACTIVE_RUN = new Set(["starting", "running"]);
const FINAL_TASK = new Set(["done", "cancelled", "merged"]);

export default function TaskDetail({ id }: { id: string }) {
  const q = useQuery(() => api.taskGet(id), [id]);
  const [tab, setTab] = useState<Tab>("conversation");
  const [runId, setRunId] = useState<string | null>(null);
  const [dialog, setDialog] = useState<null | "stop" | "cancel">(null);
  const [sent, setSent] = useState<{ ts: string; text: string }[]>([]);
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
      // Findings and checks are written when a run finishes.
      if (!ACTIVE_RUN.has(r.state)) q.reload();
    } else if ((ev.event === "approval.created" || ev.event === "approval.decided") && ev.data.task_id === id) {
      q.reload();
    }
  }, ["task.updated", "run.updated", "approval.created", "approval.decided"]);

  const d = q.data;
  const runs = useMemo(() => [...(d?.runs ?? [])].sort((a, b) => a.started_at.localeCompare(b.started_at)), [d?.runs]);
  const activeRun = runs.find((r) => ACTIVE_RUN.has(r.state));
  const selectedRun = runs.find((r) => r.id === runId) ?? activeRun ?? runs.at(-1);

  if (q.error && !d) return <div style={{ padding: "0 28px" }}><ErrorNote error={q.error} onRetry={q.reload} /></div>;
  if (!d) return <div style={{ padding: "0 28px" }}><Loading what="Loading task" /></div>;
  const t = d.task;

  const doRetry = async () => {
    const r = await act.run(() => api.taskRetry(t.id));
    if (r) toast("Retry queued with a fresh agent session.");
  };
  const doTerminal = async () => {
    const r = await act.run(() => api.taskOpenTerminal(t.id));
    if (r) toast(r.opened ? "Opened the worktree in Terminal." : "Terminal could not be opened; check that the worktree exists.", r.opened ? undefined : "bad");
  };
  const doSend = async (text: string) => {
    // Stamp before sending so the bubble sorts ahead of the run it starts.
    const ts = new Date().toISOString();
    const r = await act.run(() => api.taskMessage(t.id, text));
    if (r) {
      setSent((s) => [...s, { ts, text }]);
      q.set((x) => (x ? { ...x, task: r } : x));
      return true;
    }
    return false;
  };

  const pendingApprovals = d.approvals.filter((a) => a.state === "pending").length;
  const openFindings = d.findings.filter((f) => f.status === "open").length;
  const failedChecks = d.verifications.filter((v) => v.status === "failed" || v.status === "blocked").length;
  const final = FINAL_TASK.has(t.state);
  const count = (n: number, bad?: boolean) => (n ? <span className={`tab-count ${bad ? "bad" : ""}`}>{n}</span> : null);

  return (
    <div className="workspace">
      <header className="ws-head">
        <div className="ws-title">
          <h1>{t.title}</h1>
          <div className="ws-controls" role="toolbar" aria-label="Task controls">
            <button className="icon-btn" disabled={act.busy} onClick={() => void doTerminal()} title="Open worktree in Terminal">
              <Icon name="terminal" />
            </button>
            <button className="icon-btn" disabled={!!activeRun || act.busy || t.state === "done"} onClick={() => void doRetry()} title="Retry with a fresh session">
              <Icon name="refresh" />
            </button>
            <button className="icon-btn danger" disabled={final || act.busy} onClick={() => setDialog("cancel")} title="Cancel task">
              <Icon name="x" />
            </button>
          </div>
        </div>
        <div className="ws-meta">
          <StateChip state={t.state} />
          <RiskChip risk={t.risk} />
          <Chip>{t.category}</Chip>
          <Chip title="Priority (1 = most urgent)">P{t.priority}</Chip>
          {t.jira_key && <Chip tone="accent">{t.jira_key}</Chip>}
          <span className="sep">·</span>
          <span>{t.project}</span>
          {d.branch && (
            <>
              <span className="sep">·</span>
              <span className="mono" style={{ display: "inline-flex", gap: 4, alignItems: "center" }}>
                <Icon name="branch" size={12} />
                {d.branch}
              </span>
            </>
          )}
          <span className="sep">·</span>
          <span>attempt {t.attempts}</span>
          {t.pr_url && <Ext url={t.pr_url}>Pull request</Ext>}
          {t.source_url && <Ext url={t.source_url}>Source ({t.source})</Ext>}
        </div>
        {t.blocked_reason && (
          <div className={`note ${t.state === "blocked" || t.state === "escalated" ? "bad" : "warn"}`} role="status">
            <span>
              <Icon name="alert" size={14} /> {t.blocked_reason}
            </span>
            {t.state === "awaiting_approval" && !t.pr_url ? (
              <button className="btn small" disabled={act.busy} onClick={() => void doTerminal()}>
                <Icon name="terminal" size={13} /> Review in Terminal
              </button>
            ) : (
              t.state !== "awaiting_approval" &&
              !final && (
                <button className="btn small" disabled={act.busy} onClick={() => void doRetry()}>
                  Retry
                </button>
              )
            )}
          </div>
        )}
        <ErrorNote error={act.error} />
      </header>

      <div className="ws-tabs">
        <Tabs
          ariaLabel="Task sections"
          value={tab}
          onChange={setTab}
          tabs={[
            { key: "conversation", label: <>Conversation {activeRun && <span className="dot live" />}</> },
            { key: "diff", label: "Changes" },
            { key: "checks", label: <>Checks {count(failedChecks, true)}</> },
            { key: "findings", label: <>Review {count(openFindings)}</> },
            { key: "approvals", label: <>Approvals {count(pendingApprovals, true)}</> },
            { key: "deploys", label: "Deploys" },
            { key: "brief", label: "Details" },
            { key: "log", label: "Raw log" },
          ]}
        />
      </div>

      {tab === "conversation" ? (
        <Conversation
          d={d}
          runs={runs}
          activeRun={activeRun}
          sent={sent}
          busy={act.busy}
          onSend={doSend}
          onStop={() => setDialog("stop")}
        />
      ) : (
        <section className="ws-panel" role="tabpanel" aria-label={tab}>
          {tab === "log" &&
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
          {tab === "brief" && <Brief d={d} runs={runs} onOpenRun={(r) => (setRunId(r.id), setTab("log"))} />}
        </section>
      )}

      {dialog === "stop" && activeRun && (
        <StopDialog run={activeRun} onClose={() => setDialog(null)} onDone={(r) => (setDialog(null), toast(`Run ${r.id} ${r.state}.`))} />
      )}
      {dialog === "cancel" && (
        <CancelDialog taskId={t.id} onClose={() => setDialog(null)} onDone={() => (setDialog(null), q.reload())} />
      )}
    </div>
  );
}

// ------------------------------------------------------------ conversation

const CONV_PAGE = 5000;
const CONV_MAX = 20000;

type Item =
  | { k: "run"; run: Run }
  | { k: "text"; key: string; text: string }
  | { k: "tool"; key: string; name: string; arg: string; input?: RunEvent; result?: RunEvent; denied?: boolean }
  | { k: "note"; key: string; text: string }
  | { k: "stderr"; key: string; lines: string[] }
  | { k: "result"; key: string; run: Run }
  | { k: "founder"; key: string; text: string; ts: string };

function evId(e: RunEvent): string | undefined {
  const d = e.data as { id?: unknown } | null | undefined;
  return d && typeof d.id === "string" ? d.id : undefined;
}

function buildItems(runs: Run[], events: Record<string, RunEvent[]>, sent: { ts: string; text: string }[]): Item[] {
  const out: Item[] = [];
  const founder = [...sent];
  for (const run of runs) {
    while (founder.length && founder[0].ts <= run.started_at) {
      const f = founder.shift()!;
      out.push({ k: "founder", key: `f${f.ts}`, text: f.text, ts: f.ts });
    }
    out.push({ k: "run", run });
    const tools = new Map<string, Extract<Item, { k: "tool" }>>();
    let stderr: Extract<Item, { k: "stderr" }> | null = null;
    for (const e of events[run.id] ?? []) {
      const key = `${run.id}:${e.seq}`;
      if (e.kind !== "stderr") stderr = null;
      switch (e.kind) {
        case "system":
          break;
        case "text":
          out.push({ k: "text", key, text: e.text });
          break;
        case "tool_use": {
          const i = e.text.indexOf(": ");
          const name = (e.data as { name?: string } | undefined)?.name ?? (i > 0 ? e.text.slice(0, i) : e.text);
          const item = { k: "tool" as const, key, name, arg: i > 0 ? e.text.slice(i + 2) : "", input: e };
          const id = evId(e);
          if (id) tools.set(id, item);
          out.push(item);
          break;
        }
        case "tool_result": {
          const id = evId(e);
          const t = id ? tools.get(id) : undefined;
          if (t) t.result = e;
          else out.push({ k: "tool", key, name: "Result", arg: e.text.split("\n")[0], result: e });
          break;
        }
        case "denied":
          out.push({ k: "tool", key, name: "Blocked by policy", arg: e.text, input: e, denied: true });
          break;
        case "stderr":
          if (stderr) stderr.lines.push(e.text);
          else {
            stderr = { k: "stderr", key, lines: [e.text] };
            out.push(stderr);
          }
          break;
        case "result":
          out.push({ k: "result", key, run });
          break;
        default:
          out.push({ k: "note", key, text: e.text });
      }
    }
    // Some runs (reviews, manager calls) keep no event stream: still show the outcome.
    if (!ACTIVE_RUN.has(run.state) && !out.some((x) => x.k === "result" && x.run.id === run.id)) {
      out.push({ k: "result", key: `${run.id}:end`, run });
    }
  }
  for (const f of founder) out.push({ k: "founder", key: `f${f.ts}`, text: f.text, ts: f.ts });
  return out;
}

function Conversation({
  d,
  runs,
  activeRun,
  sent,
  busy,
  onSend,
  onStop,
}: {
  d: Detail;
  runs: Run[];
  activeRun?: Run;
  sent: { ts: string; text: string }[];
  busy: boolean;
  onSend: (text: string) => Promise<boolean>;
  onStop: () => void;
}) {
  const [events, setEvents] = useState<Record<string, RunEvent[]>>({});
  const [error, setError] = useState<string | null>(null);
  const loaded = useRef(new Set<string>());
  const pending = useRef<RunEvent[]>([]);
  const frame = useRef(0);
  const scroller = useRef<HTMLDivElement>(null);
  const stick = useRef(true);

  // Fetch every run's history once; live runs keep appending via events.
  useEffect(() => {
    for (const r of runs) {
      if (loaded.current.has(r.id)) continue;
      loaded.current.add(r.id);
      void (async () => {
        try {
          const all: RunEvent[] = [];
          for (;;) {
            const after = all.length ? all[all.length - 1].seq : undefined;
            const page = await api.runEvents(r.id, after, CONV_PAGE);
            all.push(...page);
            if (page.length < CONV_PAGE || all.length >= CONV_MAX) break;
          }
          setEvents((prev) => {
            const live = prev[r.id] ?? [];
            const last = all.length ? all[all.length - 1].seq : -1;
            return { ...prev, [r.id]: [...all, ...live.filter((e) => e.seq > last)] };
          });
        } catch (e) {
          setError(e instanceof Error ? e.message : String(e));
        }
      })();
    }
  }, [runs]);

  const flush = useCallback(() => {
    frame.current = 0;
    const batch = pending.current;
    pending.current = [];
    if (!batch.length) return;
    setEvents((prev) => {
      const next = { ...prev };
      for (const e of batch) {
        const list = next[e.run_id] ?? [];
        if (list.length && list[list.length - 1].seq >= e.seq) continue;
        next[e.run_id] = [...list, e];
      }
      return next;
    });
  }, []);

  useTpdEvents((ev) => {
    if (ev.event !== "run.event" || !runs.some((r) => r.id === ev.data.run_id)) return;
    pending.current.push(ev.data);
    if (!frame.current) frame.current = requestAnimationFrame(flush);
  }, ["run.event"]);
  useEffect(() => () => cancelAnimationFrame(frame.current), []);

  const items = useMemo(() => buildItems(runs, events, sent), [runs, events, sent]);

  useLayoutEffect(() => {
    const el = scroller.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [items, activeRun?.id]);

  return (
    <>
      <div
        className="transcript"
        ref={scroller}
        onScroll={(e) => {
          const el = e.currentTarget;
          stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
        }}
        role="log"
        aria-label="Agent conversation"
      >
        <div className="transcript-inner">
          <div className="t-founder" style={{ alignSelf: "flex-end" }}>
            <b>{d.task.title}</b>
            {d.body && d.body !== d.task.title && <div className="muted" style={{ marginTop: 4 }}>{d.body}</div>}
            {d.acceptance.length > 0 && (
              <ul className="mini-list muted small" style={{ marginTop: 6 }}>
                {d.acceptance.map((a, i) => (
                  <li key={i}>{a}</li>
                ))}
              </ul>
            )}
          </div>
          {runs.length === 0 && (
            <div className="t-system">Waiting to start — the task runs once it is approved and a slot is free.</div>
          )}
          <ErrorNote error={error} />
          {items.map((it) => renderItem(it))}
          {activeRun && (
            <div className="t-thinking">
              <span className="spinner" /> {label(activeRun.kind)} agent is working…
            </div>
          )}
        </div>
      </div>
      <Dock activeRun={activeRun} busy={busy} disabled={FINAL_TASK.has(d.task.state)} onSend={onSend} onStop={onStop} />
    </>
  );
}

function renderItem(it: Item): ReactNode {
  switch (it.k) {
    case "run":
      return (
        <div key={`run-${it.run.id}`} className="t-system">
          <Chip tone={runTone(it.run.state)}>{label(it.run.kind)}</Chip>
          {it.run.model ?? it.run.runtime} · {fmtTime(it.run.started_at)}
        </div>
      );
    case "text":
      return (
        <div key={it.key} className="t-msg">
          {it.text}
        </div>
      );
    case "tool":
      return <ToolRow key={it.key} item={it} />;
    case "note":
      return (
        <div key={it.key} className="t-system">
          {it.text}
        </div>
      );
    case "stderr":
      return <StderrBlock key={it.key} lines={it.lines} />;
    case "result":
      return (
        <div key={it.key} className="t-result">
          <div className="t-result-head">
            <Icon name={it.run.state === "succeeded" ? "check" : "alert"} size={15} className={it.run.state === "succeeded" ? "ok-text" : "bad-text"} />
            {label(it.run.kind)} run {it.run.state}
          </div>
          {it.run.summary && (
            <div className="pre">
              {it.run.summary} <span className="muted small">(claimed by agent)</span>
            </div>
          )}
          {it.run.error && <div className="bad-text pre">{it.run.error}</div>}
          <div className="t-stats">
            {[
              it.run.num_turns != null && `${it.run.num_turns} turns`,
              it.run.input_tokens != null && `${fmtNum(it.run.input_tokens)} in / ${fmtNum(it.run.output_tokens)} out`,
              it.run.cost_usd != null && `${fmtUsd(it.run.cost_usd)} reported`,
              it.run.ended_at && `${fmtMs(new Date(it.run.ended_at).getTime() - new Date(it.run.started_at).getTime())}`,
            ]
              .filter(Boolean)
              .join(" · ")}
          </div>
        </div>
      );
    case "founder":
      return (
        <div key={it.key} className="t-founder">
          {it.text}
        </div>
      );
  }
}

function ToolRow({ item }: { item: Extract<Item, { k: "tool" }> }) {
  const [open, setOpen] = useState(false);
  const failed = item.denied || (item.result?.data as { is_error?: boolean } | undefined)?.is_error;
  return (
    <div className={`t-tool ${failed ? "denied" : item.result ? "ok" : ""}`}>
      <button onClick={() => setOpen((o) => !o)} aria-expanded={open}>
        <span className="t-status">
          {failed ? <Icon name="x" size={13} /> : item.result ? <Icon name="check" size={13} /> : <Icon name="wrench" size={13} />}
        </span>
        <span className="t-name">{item.name}</span>
        <span className="t-arg">{item.arg}</span>
        <Icon name={open ? "chevronDown" : "chevronRight"} size={13} className="muted" />
      </button>
      {open && (
        <div className="t-tool-body">
          {item.input && (
            <>
              <div className="t-label">{item.denied ? "Policy decision" : "Call"}</div>
              <pre>{item.input.text}</pre>
            </>
          )}
          {item.result && (
            <>
              <div className="t-label">Result</div>
              <pre>{item.result.text || "(empty)"}</pre>
            </>
          )}
          {!item.result && !item.denied && <div className="muted small">No result yet.</div>}
        </div>
      )}
    </div>
  );
}

function StderrBlock({ lines }: { lines: string[] }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="t-tool">
      <button onClick={() => setOpen((o) => !o)} aria-expanded={open}>
        <span className="t-status">
          <Icon name="terminal" size={13} />
        </span>
        <span className="t-name muted">Output</span>
        <span className="t-arg">{lines.length} line{lines.length === 1 ? "" : "s"}</span>
        <Icon name={open ? "chevronDown" : "chevronRight"} size={13} className="muted" />
      </button>
      {open && (
        <div className="t-tool-body">
          <pre>{lines.join("\n")}</pre>
        </div>
      )}
    </div>
  );
}

function Dock({
  activeRun,
  busy,
  disabled,
  onSend,
  onStop,
}: {
  activeRun?: Run;
  busy: boolean;
  disabled: boolean;
  onSend: (text: string) => Promise<boolean>;
  onStop: () => void;
}) {
  const [text, setText] = useState("");
  const box = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    const el = box.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
  }, [text]);
  const submit = async (e?: FormEvent) => {
    e?.preventDefault();
    const v = text.trim();
    if (!v || busy || disabled) return;
    if (await onSend(v)) setText("");
  };
  return (
    <div className="dock">
      <form className="composer" onSubmit={submit}>
        <textarea
          ref={box}
          rows={1}
          value={text}
          disabled={disabled}
          placeholder={disabled ? "This task is finished." : activeRun ? "Interrupt and redirect the agent…" : "Message the agent to continue its session…"}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
              e.preventDefault();
              void submit();
            }
          }}
          aria-label="Message to agent"
        />
        <div className="composer-bar">
          <span className="muted small">{activeRun ? `${label(activeRun.kind)} run ${activeRun.id} is live` : "Resumes the task's session; audited as an intervention"}</span>
          {activeRun && !text.trim() ? (
            <button type="button" className="send stop" onClick={onStop} title="Stop the run" aria-label="Stop run">
              <Icon name="stop" size={14} />
            </button>
          ) : (
            <button type="submit" className="send" disabled={!text.trim() || busy || disabled} aria-label="Send message">
              {busy ? <span className="spinner" style={{ borderTopColor: "#fff" }} /> : <Icon name="arrowUp" />}
            </button>
          )}
        </div>
      </form>
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
