// Memory: scoped lessons with provenance. Proposed items only take effect
// once validated.

import { useState, type FormEvent } from "react";
import { api, useAction, useQuery } from "../api";
import { useApp } from "../router";
import type { MemoryItem, MemoryStatus } from "../types";
import { Chip, Empty, ErrorNote, Field, fmtTime, JsonBlock, Loading, Modal, PageHeader, statusTone } from "../ui";

const CATEGORIES = ["convention", "pitfall", "preference", "architecture", "process", "other"];

export default function Memory() {
  const app = useApp();
  const [scope, setScope] = useState(app.project || "");
  const [status, setStatus] = useState<MemoryStatus | "">("");
  const [search, setSearch] = useState("");
  const [active, setActive] = useState(""); // submitted search
  const [proposing, setProposing] = useState(false);

  const q = useQuery(
    () => (active ? api.memorySearch(active, scope || undefined) : api.memoryList(scope || undefined, status || undefined)),
    [scope, status, active],
  );

  const update = (m: MemoryItem) => q.set((prev) => prev?.map((x) => (x.id === m.id ? m : x)));
  const rows = (q.data ?? []).filter((m) => !active || !status || m.status === status);

  return (
    <div className="page">
      <PageHeader
        title="Memory"
        sub="Lessons the factory carries between runs. Only validated items are given to agents."
        actions={
          <button className="btn primary" onClick={() => setProposing(true)}>
            Propose memory
          </button>
        }
      />
      <form
        className="filters"
        role="search"
        onSubmit={(e) => {
          e.preventDefault();
          setActive(search.trim());
        }}
      >
        <select value={scope} onChange={(e) => setScope(e.target.value)} aria-label="Scope">
          <option value="">All scopes</option>
          <option value="global">global</option>
          {app.projects.map((p) => (
            <option key={p.slug} value={p.slug}>
              {p.slug}
            </option>
          ))}
        </select>
        <select value={status} onChange={(e) => setStatus(e.target.value as MemoryStatus | "")} aria-label="Status">
          <option value="">Any status</option>
          <option value="proposed">proposed</option>
          <option value="validated">validated</option>
          <option value="retired">retired</option>
        </select>
        <input type="search" value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search memory" aria-label="Search memory" />
        <button className="btn" type="submit">
          Search
        </button>
        {active && (
          <button
            className="btn ghost"
            type="button"
            onClick={() => {
              setSearch("");
              setActive("");
            }}
          >
            Clear search
          </button>
        )}
      </form>
      <ErrorNote error={q.error} onRetry={q.reload} />
      {q.loading && !q.data ? (
        <Loading />
      ) : rows.length === 0 ? (
        <Empty>{active ? "No matches." : "No memory items."}</Empty>
      ) : (
        <div className="stack">
          {rows.map((m) => (
            <MemoryRow key={m.id} m={m} onChange={update} />
          ))}
        </div>
      )}
      {proposing && (
        <Propose
          defaultScope={scope || "global"}
          onClose={() => setProposing(false)}
          onDone={(m) => {
            setProposing(false);
            q.set((prev) => [m, ...(prev ?? [])]);
          }}
        />
      )}
    </div>
  );
}

function MemoryRow({ m, onChange }: { m: MemoryItem; onChange: (m: MemoryItem) => void }) {
  const act = useAction();
  const [open, setOpen] = useState(false);
  const set = async (s: MemoryStatus) => {
    const r = await act.run(() => api.memorySetStatus(m.id, s));
    if (r) onChange(r);
  };
  return (
    <article className={`memory ${m.status}`}>
      <div className="memory-head">
        <Chip tone={statusTone(m.status)}>{m.status}</Chip>
        <Chip>{m.scope}</Chip>
        <span className="muted small">{m.category}</span>
        <span className="muted small">confidence {Math.round(m.confidence * 100)}%</span>
        <span className="muted small">
          v{m.version} · {fmtTime(m.updated_at)}
        </span>
        <span className="spacer" />
        {m.status !== "validated" && m.status !== "retired" && (
          <button className="btn small primary" disabled={act.busy} onClick={() => void set("validated")}>
            Validate
          </button>
        )}
        {m.status !== "retired" && (
          <button className="btn small" disabled={act.busy} onClick={() => void set("retired")}>
            Retire
          </button>
        )}
        <button className="btn small ghost" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
          Provenance
        </button>
      </div>
      <p className="memory-text">{m.text}</p>
      {open && <JsonBlock value={m.provenance} />}
      <ErrorNote error={act.error} />
    </article>
  );
}

function Propose({
  defaultScope,
  onClose,
  onDone,
}: {
  defaultScope: string;
  onClose: () => void;
  onDone: (m: MemoryItem) => void;
}) {
  const { projects } = useApp();
  const [scope, setScope] = useState(defaultScope);
  const [category, setCategory] = useState(CATEGORIES[0]);
  const [text, setText] = useState("");
  const act = useAction();
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const r = await act.run(() => api.memoryPropose({ scope, category, text: text.trim() }));
    if (r) onDone(r);
  };
  return (
    <Modal title="Propose memory" onClose={onClose}>
      <form className="form" onSubmit={(e) => void submit(e)}>
        <div className="form-row">
          <Field label="Scope">
            <select value={scope} onChange={(e) => setScope(e.target.value)}>
              <option value="global">global</option>
              {projects.map((p) => (
                <option key={p.slug} value={p.slug}>
                  {p.slug}
                </option>
              ))}
            </select>
          </Field>
          <Field label="Category">
            <input value={category} onChange={(e) => setCategory(e.target.value)} list="mem-cats" required />
            <datalist id="mem-cats">
              {CATEGORIES.map((c) => (
                <option key={c} value={c} />
              ))}
            </datalist>
          </Field>
        </div>
        <Field label="Lesson" hint="One concrete, checkable statement. Never include secrets.">
          <textarea rows={5} value={text} onChange={(e) => setText(e.target.value)} required />
        </Field>
        <ErrorNote error={act.error} />
        <div className="modal-foot">
          <button type="button" className="btn" onClick={onClose}>
            Cancel
          </button>
          <button type="submit" className="btn primary" disabled={!text.trim() || act.busy}>
            Propose
          </button>
        </div>
      </form>
    </Modal>
  );
}
