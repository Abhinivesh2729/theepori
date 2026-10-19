// Settings & Safety: emergency controls, export/import, doctor, audit log.

import { useEffect, useRef, useState, type FormEvent } from "react";
import { api, useAction, useQuery, useTpdEvents } from "../api";
import { useApp, type Route } from "../router";
import type { AuditRecord, ExportReport, ImportReport, Mode } from "../types";
import { VirtualList } from "../VirtualList";
import { Card, Chip, Confirm, Empty, ErrorNote, Field, fmtNum, Loading, PageHeader, statusTone, Toggle } from "../ui";
import { AuditRow } from "../widgets";

export default function Settings({ route }: { route: Route }) {
  const auditRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (route.id === "audit") auditRef.current?.scrollIntoView({ block: "start" });
  }, [route.id]);
  return (
    <div className="page">
      <PageHeader title="Settings & safety" sub="Emergency controls apply immediately and are audited. The same controls exist in the CLI." />
      <div className="grid two">
        <Controls />
        <StopAll />
        <Export />
        <Import />
      </div>
      <Doctor />
      <div ref={auditRef}>
        <AuditLog />
      </div>
    </div>
  );
}

const MODES: { key: Mode; title: string; body: string }[] = [
  { key: "normal", title: "Normal", body: "Assignments, merges and deploys follow policy." },
  { key: "paused", title: "Paused", body: "No new assignments; running work continues." },
  { key: "safe", title: "Safe mode", body: "Paused, running sessions stopped, no merges or deploys." },
];

function Controls() {
  const { refreshStatus } = useApp();
  const q = useQuery(() => api.controls(), []);
  const act = useAction();
  useTpdEvents((ev) => ev.event === "controls.changed" && q.set(ev.data), ["controls.changed"]);
  const c = q.data;
  const set = async (u: Parameters<typeof api.setControls>[0]) => {
    const r = await act.run(() => api.setControls(u));
    if (r) {
      q.set(r);
      refreshStatus();
    }
  };
  return (
    <Card title="Factory controls">
      <ErrorNote error={q.error} onRetry={q.reload} />
      {!c ? (
        q.loading && <Loading />
      ) : (
        <>
          <fieldset className="modes" disabled={act.busy}>
            <legend className="field-label">Mode</legend>
            {MODES.map((m) => (
              <label key={m.key} className={`mode ${c.mode === m.key ? "on" : ""} mode-${m.key}`}>
                <input type="radio" name="mode" checked={c.mode === m.key} onChange={() => void set({ mode: m.key })} />
                <span>
                  <b>{m.title}</b>
                  <span className="muted small">{m.body}</span>
                </span>
              </label>
            ))}
          </fieldset>
          <Toggle
            label="Auto-merge"
            hint="Merge when the gate passes and risk is within the repo's policy."
            checked={c.auto_merge}
            disabled={act.busy}
            onChange={(v) => void set({ auto_merge: v })}
          />
          <Toggle
            label="Auto-deploy"
            hint="Deploy merged changes without a separate approval where policy allows."
            checked={c.auto_deploy}
            disabled={act.busy}
            onChange={(v) => void set({ auto_deploy: v })}
          />
          <Toggle
            label="Remote access"
            hint="Allow paired phones to reach tpd through the relay. The mobile app is not built yet."
            checked={c.remote_access}
            disabled={act.busy}
            onChange={(v) => void set({ remote_access: v })}
          />
          <ErrorNote error={act.error} />
        </>
      )}
    </Card>
  );
}

function StopAll() {
  const [open, setOpen] = useState(false);
  const [result, setResult] = useState<string | null>(null);
  const act = useAction();
  const { refreshStatus } = useApp();
  return (
    <Card title="Emergency stop" className="danger-card">
      <p>Stops every running agent session now. Tasks keep their state and can be retried. Combine with Safe mode to keep them stopped.</p>
      <button className="btn danger big" onClick={() => setOpen(true)}>
        Stop all agents
      </button>
      {result && (
        <p className="small" role="status">
          {result}
        </p>
      )}
      {open && (
        <Confirm
          title="Stop all agents?"
          body={<p>Every running session is interrupted immediately. This is audited.</p>}
          confirmLabel="Stop all agents"
          danger
          busy={act.busy}
          error={act.error}
          onClose={() => setOpen(false)}
          onConfirm={() =>
            void act.run(() => api.stopAll()).then((r) => {
              if (r) {
                setOpen(false);
                setResult(`Stopped ${r.stopped} run${r.stopped === 1 ? "" : "s"}.`);
                refreshStatus();
              }
            })
          }
        />
      )}
    </Card>
  );
}

const VAULT_PHRASE = "include encrypted vault";

function Export() {
  const [path, setPath] = useState("");
  const [vault, setVault] = useState(false);
  const [phrase, setPhrase] = useState("");
  const [days, setDays] = useState(90);
  const [report, setReport] = useState<ExportReport | null>(null);
  const act = useAction();
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const r = await act.run(() =>
      api.exportCreate({
        path: path.trim(),
        include_vault: vault,
        vault_confirmation: vault ? phrase : undefined,
        audit_days: days,
      }),
    );
    if (r) setReport(r);
  };
  return (
    <Card title="Export">
      <form className="form" onSubmit={(e) => void submit(e)}>
        <Field label="Archive path">
          <input className="mono" value={path} onChange={(e) => setPath(e.target.value)} placeholder="~/Desktop/factory-export.tar.zst" required />
        </Field>
        <Field label="Audit history (days)">
          <input type="number" min={0} value={days} onChange={(e) => setDays(Number(e.target.value))} />
        </Field>
        <label className="check">
          <input type="checkbox" checked={vault} onChange={(e) => setVault(e.target.checked)} />
          Include the encrypted vault
        </label>
        {vault && (
          <Field label={`Type "${VAULT_PHRASE}" to confirm`} hint="The vault stays encrypted with your master password.">
            <input value={phrase} onChange={(e) => setPhrase(e.target.value)} autoComplete="off" />
          </Field>
        )}
        <ErrorNote error={act.error} />
        <button className="btn primary" type="submit" disabled={act.busy || !path.trim() || (vault && phrase !== VAULT_PHRASE)}>
          {act.busy ? "Exporting…" : "Export"}
        </button>
      </form>
      {report && (
        <dl className="kv small">
          <dt>Written</dt>
          <dd className="mono">{report.path}</dd>
          <dt>Size</dt>
          <dd>{fmtNum(report.bytes)} bytes</dd>
          <dt>SHA-256</dt>
          <dd className="mono">{report.sha256}</dd>
          <dt>Included</dt>
          <dd>{report.included.join(", ") || "–"}</dd>
          <dt>Excluded</dt>
          <dd>{report.excluded.join(", ") || "–"}</dd>
          <dt>Reconnect after import</dt>
          <dd>{report.reconnect.join(", ") || "–"}</dd>
        </dl>
      )}
    </Card>
  );
}

function Import() {
  const [path, setPath] = useState("");
  const [preview, setPreview] = useState<ImportReport | null>(null);
  const [confirm, setConfirm] = useState(false);
  const act = useAction();
  const apply = useAction();
  return (
    <Card title="Import">
      <form
        className="form"
        onSubmit={(e) => {
          e.preventDefault();
          void act.run(() => api.importPreview(path.trim())).then((r) => r && setPreview(r));
        }}
      >
        <Field label="Archive path">
          <input
            className="mono"
            value={path}
            onChange={(e) => {
              setPath(e.target.value);
              setPreview(null);
            }}
            required
          />
        </Field>
        <ErrorNote error={act.error} />
        <div className="row">
          <button className="btn" type="submit" disabled={act.busy || !path.trim()}>
            Preview
          </button>
          <button
            className="btn primary"
            type="button"
            disabled={!preview || preview.applied || apply.busy}
            onClick={() => setConfirm(true)}
          >
            Apply import…
          </button>
        </div>
      </form>
      {preview && <ImportSummary r={preview} />}
      {confirm && preview && (
        <Confirm
          title="Apply import?"
          body={
            <p>
              Adds {preview.projects.length} projects and {preview.memory_new} memory items. Duplicates are skipped; conflicts are kept as
              proposals. Integrations listed under “reconnect” need to be set up again.
            </p>
          }
          confirmLabel="Apply"
          busy={apply.busy}
          error={apply.error}
          onClose={() => setConfirm(false)}
          onConfirm={() =>
            void apply.run(() => api.importApply(preview.path)).then((r) => {
              if (r) {
                setPreview(r);
                setConfirm(false);
              }
            })
          }
        />
      )}
    </Card>
  );
}

function ImportSummary({ r }: { r: ImportReport }) {
  return (
    <dl className="kv small">
      <dt>Status</dt>
      <dd>{r.applied ? <Chip tone="ok">applied</Chip> : <Chip tone="info">preview</Chip>} schema {r.schema}</dd>
      <dt>Projects</dt>
      <dd>{r.projects.join(", ") || "–"}</dd>
      <dt>Memory</dt>
      <dd>
        {r.memory_new} new · {r.memory_duplicates} duplicates · {r.memory_conflicts.length} conflicts
      </dd>
      {r.memory_conflicts.length > 0 && (
        <>
          <dt>Conflicts</dt>
          <dd>{r.memory_conflicts.join("; ")}</dd>
        </>
      )}
      <dt>Files</dt>
      <dd>{r.files.length}</dd>
      <dt>Reconnect</dt>
      <dd>{r.reconnect.join(", ") || "–"}</dd>
      {r.warnings.length > 0 && (
        <>
          <dt>Warnings</dt>
          <dd className="warn-text">{r.warnings.join("; ")}</dd>
        </>
      )}
    </dl>
  );
}

function Doctor() {
  const [run, setRun] = useState(0);
  const q = useQuery(() => (run ? api.doctor() : Promise.resolve(null)), [run]);
  return (
    <Card
      title="Doctor"
      actions={
        <button className="btn" onClick={() => setRun((n) => n + 1)} disabled={q.loading && run > 0}>
          {run ? "Run again" : "Run checks"}
        </button>
      }
    >
      <ErrorNote error={q.error} />
      {!run ? (
        <p className="muted">Checks the factory layout, daemon, database, audit chain, Claude Code and gh.</p>
      ) : q.loading && !q.data ? (
        <Loading what="Running checks" />
      ) : q.data ? (
        <>
          <p>
            Overall <Chip tone={statusTone(q.data.status)}>{q.data.status}</Chip>
          </p>
          <table className="dense">
            <thead>
              <tr>
                <th>Check</th>
                <th>Status</th>
                <th>Detail</th>
                <th>Fix</th>
              </tr>
            </thead>
            <tbody>
              {q.data.checks.map((c) => (
                <tr key={c.name}>
                  <td>{c.name}</td>
                  <td>
                    <Chip tone={statusTone(c.status)}>{c.status}</Chip>
                  </td>
                  <td>{c.detail}</td>
                  <td className="mono small">{c.fix ?? ""}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </>
      ) : null}
    </Card>
  );
}

const AUDIT_ROW_H = 28;

function AuditLog() {
  const [limit, setLimit] = useState(500);
  const q = useQuery(() => api.auditTail(limit).then((a) => [...a].reverse()), [limit]);
  const verify = useAction();
  const [verified, setVerified] = useState<string | null>(null);
  const [filter, setFilter] = useState("");

  useTpdEvents((ev) => {
    if (ev.event === "audit.appended") q.set((prev) => (prev ? [ev.data as AuditRecord, ...prev].slice(0, limit) : prev));
  }, ["audit.appended"]);

  const f = filter.trim().toLowerCase();
  const rows = (q.data ?? []).filter(
    (r) =>
      !f ||
      r.action.toLowerCase().includes(f) ||
      (r.project ?? "").toLowerCase().includes(f) ||
      (r.task ?? "").toLowerCase().includes(f) ||
      r.actor.id.toLowerCase().includes(f),
  );

  return (
    <Card
      title="Audit log"
      count={rows.length}
      actions={
        <>
          <input type="search" value={filter} onChange={(e) => setFilter(e.target.value)} placeholder="Filter" aria-label="Filter audit" />
          <select value={limit} onChange={(e) => setLimit(Number(e.target.value))} aria-label="Records">
            {[100, 500, 2000, 10000].map((n) => (
              <option key={n} value={n}>
                last {n}
              </option>
            ))}
          </select>
          <button
            className="btn"
            disabled={verify.busy}
            onClick={() =>
              void verify
                .run(() => api.auditVerify())
                .then((r) => r && setVerified(r.ok ? `Chain intact · ${r.records} records` : `Chain broken: ${r.problem ?? "unknown problem"}`))
            }
          >
            Verify chain
          </button>
        </>
      }
    >
      {verified && (
        <div className={`note ${verified.startsWith("Chain intact") ? "ok" : "bad"}`} role="status">
          {verified}
        </div>
      )}
      <ErrorNote error={verify.error ?? q.error} />
      {q.loading && !q.data ? (
        <Loading />
      ) : rows.length === 0 ? (
        <Empty>No records.</Empty>
      ) : (
        <div className="audit virtual">
          <VirtualList
            count={rows.length}
            rowHeight={AUDIT_ROW_H}
            className="audit-list"
            ariaLabel="Audit records"
            initialHeight={420}
            renderRow={(i) => <AuditRow r={rows[i]} as="div" />}
          />
        </div>
      )}
    </Card>
  );
}
