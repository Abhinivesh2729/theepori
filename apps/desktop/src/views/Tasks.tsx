// Tasks: filterable, virtualized table + create dialog. `#/tasks/<id>` opens
// the Coding Workspace (separate chunk).

import { lazy, Suspense, useMemo, useState } from "react";
import { api, useAction, useQuery, useTpdEvents } from "../api";
import { navigate, useApp, type Route } from "../router";
import { CATEGORIES, TASK_STATES, type Category, type TaskCreate, type TaskState, type TaskSummary } from "../types";
import { VirtualList } from "../VirtualList";
import {
  Chip,
  Empty,
  ErrorNote,
  Field,
  label,
  Loading,
  Modal,
  PageHeader,
  relTime,
  RiskChip,
  StateChip,
} from "../ui";

const TaskDetail = lazy(() => import("./TaskDetail"));
const ROW_H = 38;
const LIMITS = [200, 500, 2000];

export default function Tasks({ route }: { route: Route }) {
  if (route.id) {
    return (
      <Suspense fallback={<Loading what="Loading task" />}>
        <TaskDetail id={route.id} key={route.id} />
      </Suspense>
    );
  }
  return <TaskList />;
}

type StateFilter = "open" | "all" | TaskState;

function TaskList() {
  const app = useApp();
  const [project, setProject] = useState(app.project);
  const [state, setState] = useState<StateFilter>("open");
  const [category, setCategory] = useState<Category | "">("");
  const [text, setText] = useState("");
  const [limit, setLimit] = useState(500);
  const [creating, setCreating] = useState(false);

  const tasks = useQuery(
    () =>
      api.taskList({
        project: project || undefined,
        open_only: state === "open",
        states: state !== "open" && state !== "all" ? [state] : undefined,
        limit,
      }),
    [project, state, limit],
  );

  useTpdEvents((ev) => {
    if (ev.event !== "task.updated") return;
    const t = ev.data;
    tasks.set((prev) => {
      if (!prev) return prev;
      const i = prev.findIndex((x) => x.id === t.id);
      if (i >= 0) {
        const next = prev.slice();
        next[i] = t;
        return next;
      }
      return matches(t, project, state) ? [t, ...prev] : prev;
    });
  }, ["task.updated"]);

  const rows = useMemo(() => {
    const q = text.trim().toLowerCase();
    return (tasks.data ?? []).filter(
      (t) =>
        (!category || t.category === category) &&
        (state === "all" || state === "open" || t.state === state) &&
        (!q || t.title.toLowerCase().includes(q) || t.id.toLowerCase().includes(q) || (t.jira_key ?? "").toLowerCase().includes(q)),
    );
  }, [tasks.data, category, text, state]);

  return (
    <div className="page fill">
      <PageHeader
        title="Tasks"
        actions={
          <button className="btn primary" onClick={() => setCreating(true)}>
            New task
          </button>
        }
      />
      <div className="filters" role="search" aria-label="Filter tasks">
        <select value={project} onChange={(e) => setProject(e.target.value)} aria-label="Project">
          <option value="">All projects</option>
          {app.projects.map((p) => (
            <option key={p.slug} value={p.slug}>
              {p.slug}
            </option>
          ))}
        </select>
        <select value={state} onChange={(e) => setState(e.target.value as StateFilter)} aria-label="State">
          <option value="open">Open</option>
          <option value="all">All states</option>
          {TASK_STATES.map((s) => (
            <option key={s} value={s}>
              {label(s)}
            </option>
          ))}
        </select>
        <select value={category} onChange={(e) => setCategory(e.target.value as Category | "")} aria-label="Category">
          <option value="">All categories</option>
          {CATEGORIES.map((c) => (
            <option key={c} value={c}>
              {c}
            </option>
          ))}
        </select>
        <input
          type="search"
          placeholder="Filter by title, id, Jira key"
          value={text}
          onChange={(e) => setText(e.target.value)}
          aria-label="Filter text"
        />
        <select value={limit} onChange={(e) => setLimit(Number(e.target.value))} aria-label="Limit">
          {LIMITS.map((l) => (
            <option key={l} value={l}>
              {l} max
            </option>
          ))}
        </select>
        <span className="muted small">
          {rows.length} shown{tasks.data && tasks.data.length >= limit ? " (limit reached)" : ""}
        </span>
      </div>
      <ErrorNote error={tasks.error} onRetry={tasks.reload} />
      {tasks.loading && !tasks.data ? (
        <Loading />
      ) : rows.length === 0 ? (
        <Empty>No tasks match.</Empty>
      ) : (
        <div className="vtable" role="table" aria-label="Tasks" aria-rowcount={rows.length + 1}>
          <div className="vt-row vt-head" role="row">
            <span role="columnheader">Task</span>
            <span role="columnheader">Project</span>
            <span role="columnheader">Category</span>
            <span role="columnheader">P</span>
            <span role="columnheader">Risk</span>
            <span role="columnheader">State</span>
            <span role="columnheader">Links</span>
            <span role="columnheader">Updated</span>
          </div>
          <VirtualList
            role="rowgroup"
            count={rows.length}
            rowHeight={ROW_H}
            className="vt-body"
            renderRow={(i) => <TaskRow t={rows[i]} />}
          />
        </div>
      )}
      {creating && (
        <CreateTask
          defaultProject={project || app.projects[0]?.slug || ""}
          onClose={() => setCreating(false)}
          onCreated={(t) => {
            setCreating(false);
            navigate({ view: "tasks", id: t.id });
          }}
        />
      )}
    </div>
  );
}

function matches(t: TaskSummary, project: string, state: StateFilter): boolean {
  if (project && t.project !== project) return false;
  if (state === "open") return t.state !== "done" && t.state !== "cancelled";
  return state === "all" || t.state === state;
}

function TaskRow({ t }: { t: TaskSummary }) {
  return (
    <a className="vt-row" role="row" href={`#/tasks/${encodeURIComponent(t.id)}`}>
      <span role="cell" className="vt-title" title={t.title}>
        <span className="mono muted">{t.id}</span> {t.title}
      </span>
      <span role="cell">{t.project}</span>
      <span role="cell">
        <Chip>{t.category}</Chip>
      </span>
      <span role="cell">P{t.priority}</span>
      <span role="cell">
        <RiskChip risk={t.risk} />
      </span>
      <span role="cell" title={t.blocked_reason}>
        <StateChip state={t.state} />
        {t.attempts > 1 && <span className="muted small"> ×{t.attempts}</span>}
      </span>
      <span role="cell" className="muted small">
        {t.jira_key ?? ""} {t.pr_url ? "PR" : ""}
      </span>
      <span role="cell" className="muted small">
        {relTime(t.updated_at)}
      </span>
    </a>
  );
}

export function CreateTask({
  defaultProject,
  onClose,
  onCreated,
}: {
  defaultProject: string;
  onClose: () => void;
  onCreated: (t: TaskSummary) => void;
}) {
  const { projects } = useApp();
  const [f, setF] = useState<TaskCreate>({ project: defaultProject, title: "", body: "", approve_now: false });
  const act = useAction();
  const valid = f.project && f.title.trim();

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!valid) return;
    const t = await act.run(() => api.taskCreate({ ...f, title: f.title.trim() }));
    if (t) onCreated(t);
  };

  return (
    <Modal title="New task" onClose={onClose} wide>
      <form onSubmit={(e) => void submit(e)} className="form">
        <Field label="Project">
          <select value={f.project} onChange={(e) => setF({ ...f, project: e.target.value })} required>
            <option value="" disabled>
              Choose a project
            </option>
            {projects.map((p) => (
              <option key={p.slug} value={p.slug}>
                {p.slug}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Title">
          <input value={f.title} onChange={(e) => setF({ ...f, title: e.target.value })} required maxLength={200} />
        </Field>
        <Field label="Description" hint="What should change and how we know it worked.">
          <textarea rows={6} value={f.body} onChange={(e) => setF({ ...f, body: e.target.value })} />
        </Field>
        <div className="form-row">
          <Field label="Category" hint="Blank lets the manager triage it.">
            <select
              value={f.category ?? ""}
              onChange={(e) => setF({ ...f, category: (e.target.value || undefined) as Category | undefined })}
            >
              <option value="">Auto</option>
              {CATEGORIES.map((c) => (
                <option key={c} value={c}>
                  {c}
                </option>
              ))}
            </select>
          </Field>
          <Field label="Priority" hint="1 = most urgent">
            <select
              value={f.priority ?? ""}
              onChange={(e) => setF({ ...f, priority: e.target.value ? Number(e.target.value) : undefined })}
            >
              <option value="">Auto</option>
              {[1, 2, 3, 4, 5].map((n) => (
                <option key={n} value={n}>
                  P{n}
                </option>
              ))}
            </select>
          </Field>
        </div>
        <label className="check">
          <input
            type="checkbox"
            checked={!!f.approve_now}
            onChange={(e) => setF({ ...f, approve_now: e.target.checked })}
          />
          Approve now — skip the daily plan (still policy-checked)
        </label>
        <ErrorNote error={act.error} />
        <div className="modal-foot">
          <button type="button" className="btn" onClick={onClose}>
            Cancel
          </button>
          <button type="submit" className="btn primary" disabled={!valid || act.busy}>
            {act.busy ? "Creating…" : "Create task"}
          </button>
        </div>
      </form>
    </Modal>
  );
}
