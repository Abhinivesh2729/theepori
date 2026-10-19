// Vault: secrets are write-only from this window. Values are never fetched,
// displayed or kept after a save; only metadata is listed.

import { useState, type FormEvent } from "react";
import { api, useAction, useQuery } from "../api";
import { useApp } from "../router";
import type { VaultItemMeta, VaultStatus } from "../types";
import { Card, Chip, Confirm, Empty, ErrorNote, Field, fmtTime, Loading, PageHeader } from "../ui";

export default function Vault() {
  const st = useQuery(() => api.vaultStatus(), []);
  const s = st.data;
  return (
    <div className="page">
      <PageHeader
        title="Vault"
        sub="Encrypted secrets injected into runs at runtime. Agents never read the vault directly."
        actions={
          s && (
            <span className="row">
              <Chip tone={s.unlocked ? "warn" : "ok"}>{s.unlocked ? "unlocked" : "locked"}</Chip>
              {s.unlocked && s.lease_expires_at && <span className="muted small">lease until {fmtTime(s.lease_expires_at)}</span>}
              <span className="muted small">{s.items} items</span>
            </span>
          )
        }
      />
      <ErrorNote error={st.error} onRetry={st.reload} />
      {!s ? (
        st.loading && <Loading />
      ) : !s.initialized ? (
        <Init onDone={st.set} />
      ) : !s.unlocked ? (
        <Unlock onDone={st.set} />
      ) : (
        <Unlocked status={s} onStatus={st.set} />
      )}
    </div>
  );
}

function Init({ onDone }: { onDone: (s: VaultStatus) => void }) {
  const [pw, setPw] = useState("");
  const [pw2, setPw2] = useState("");
  const act = useAction();
  const mismatch = pw2.length > 0 && pw !== pw2;
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (pw.length < 12 || pw !== pw2) return;
    const r = await act.run(() => api.vaultInit(pw));
    setPw("");
    setPw2("");
    if (r) onDone(r);
  };
  return (
    <Card title="Create the vault">
      <form className="form narrow" onSubmit={(e) => void submit(e)}>
        <p className="muted">
          Choose a strong master password. It is never stored; if you lose it the vault cannot be recovered.
        </p>
        <Field label="Master password" hint="At least 12 characters.">
          <input type="password" autoComplete="new-password" value={pw} onChange={(e) => setPw(e.target.value)} minLength={12} required />
        </Field>
        <Field label="Repeat password">
          <input
            type="password"
            autoComplete="new-password"
            value={pw2}
            onChange={(e) => setPw2(e.target.value)}
            aria-invalid={mismatch}
            required
          />
        </Field>
        {mismatch && <p className="bad-text small">Passwords do not match.</p>}
        <ErrorNote error={act.error} />
        <button className="btn primary" type="submit" disabled={act.busy || pw.length < 12 || pw !== pw2}>
          Create vault
        </button>
      </form>
    </Card>
  );
}

function Unlock({ onDone }: { onDone: (s: VaultStatus) => void }) {
  const [pw, setPw] = useState("");
  const [ttl, setTtl] = useState(30);
  const act = useAction();
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const r = await act.run(() => api.vaultUnlock(pw, ttl));
    setPw("");
    if (r) onDone(r);
  };
  return (
    <Card title="Unlock">
      <form className="form narrow" onSubmit={(e) => void submit(e)}>
        <Field label="Master password">
          <input type="password" autoComplete="current-password" value={pw} onChange={(e) => setPw(e.target.value)} required />
        </Field>
        <Field label="Lease" hint="The key is wiped from tpd's memory when the lease ends.">
          <select value={ttl} onChange={(e) => setTtl(Number(e.target.value))}>
            {[5, 15, 30, 60, 240].map((m) => (
              <option key={m} value={m}>
                {m} minutes
              </option>
            ))}
          </select>
        </Field>
        <ErrorNote error={act.error} />
        <button className="btn primary" type="submit" disabled={act.busy || !pw}>
          Unlock
        </button>
      </form>
    </Card>
  );
}

function Unlocked({ status, onStatus }: { status: VaultStatus; onStatus: (s: VaultStatus) => void }) {
  const app = useApp();
  const [project, setProject] = useState(app.project);
  const [env, setEnv] = useState("");
  const list = useQuery(() => api.vaultList(project || undefined, env || undefined), [project, env, status.items]);
  const lock = useAction();
  const [del, setDel] = useState<VaultItemMeta | null>(null);
  const delAct = useAction();

  return (
    <>
      <div className="row">
        <button
          className="btn"
          disabled={lock.busy}
          onClick={() => void lock.run(() => api.vaultLock()).then((r) => r && onStatus(r))}
        >
          Lock now
        </button>
        <ErrorNote error={lock.error} />
      </div>
      <SetSecret
        defaultProject={project || app.projects[0]?.slug || ""}
        onSaved={() => {
          list.reload();
          void api.vaultStatus().then(onStatus, () => undefined);
        }}
      />
      <Card title="Secrets" count={list.data?.length}>
        <div className="filters">
          <select value={project} onChange={(e) => setProject(e.target.value)} aria-label="Project">
            <option value="">All projects</option>
            {app.projects.map((p) => (
              <option key={p.slug} value={p.slug}>
                {p.slug}
              </option>
            ))}
          </select>
          <input value={env} onChange={(e) => setEnv(e.target.value)} placeholder="Environment" aria-label="Environment" />
        </div>
        <ErrorNote error={list.error} onRetry={list.reload} />
        {list.loading && !list.data ? (
          <Loading />
        ) : (list.data ?? []).length === 0 ? (
          <Empty>No secrets stored.</Empty>
        ) : (
          <table className="dense">
            <thead>
              <tr>
                <th>Project</th>
                <th>Environment</th>
                <th>Name</th>
                <th>Value</th>
                <th>Version</th>
                <th>Updated</th>
                <th>Expires</th>
                <th>
                  <span className="sr-only">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {(list.data ?? []).map((i) => (
                <tr key={`${i.project}/${i.environment}/${i.name}`}>
                  <td>{i.project}</td>
                  <td>{i.environment}</td>
                  <td className="mono">{i.name}</td>
                  <td className="muted" aria-label="hidden">
                    ••••••••
                  </td>
                  <td>v{i.version}</td>
                  <td className="muted">{fmtTime(i.updated_at)}</td>
                  <td>
                    {i.expires_at ? fmtTime(i.expires_at) : "–"} {i.rotation_due && <Chip tone="warn">rotate</Chip>}
                  </td>
                  <td>
                    <button className="btn small danger" onClick={() => setDel(i)} aria-label={`Delete ${i.name}`}>
                      Delete
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </Card>
      {del && (
        <Confirm
          title={`Delete ${del.name}?`}
          body={
            <p>
              Removes <span className="mono">{del.name}</span> from {del.project}/{del.environment}. Runs that need it will be
              blocked until it is set again.
            </p>
          }
          confirmLabel="Delete secret"
          danger
          typed={del.name}
          busy={delAct.busy}
          error={delAct.error}
          onClose={() => setDel(null)}
          onConfirm={() =>
            void delAct
              .run(() => api.vaultDelete({ project: del.project, environment: del.environment, name: del.name }))
              .then((r) => {
                if (r) {
                  setDel(null);
                  list.reload();
                }
              })
          }
        />
      )}
    </>
  );
}

function SetSecret({ defaultProject, onSaved }: { defaultProject: string; onSaved: () => void }) {
  const { projects } = useApp();
  const [project, setProject] = useState(defaultProject);
  const [environment, setEnvironment] = useState("dev");
  const [name, setName] = useState("");
  const [value, setValue] = useState("");
  const [saved, setSaved] = useState<string | null>(null);
  const act = useAction();

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const v = value;
    // Drop the value from UI state before the call returns.
    setValue("");
    const r = await act.run(() => api.vaultSet({ project, environment, name: name.trim(), value: v }));
    if (r) {
      setSaved(`${r.name} saved (v${r.version}).`);
      setName("");
      onSaved();
    }
  };

  return (
    <Card title="Set a secret">
      <form className="row wrap" onSubmit={(e) => void submit(e)} autoComplete="off">
        <Field label="Project">
          <select value={project} onChange={(e) => setProject(e.target.value)} required>
            <option value="" disabled>
              Choose
            </option>
            {projects.map((p) => (
              <option key={p.slug} value={p.slug}>
                {p.slug}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Environment">
          <input value={environment} onChange={(e) => setEnvironment(e.target.value)} required />
        </Field>
        <Field label="Name">
          <input
            className="mono"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="STRIPE_SECRET_KEY"
            pattern="[A-Za-z_][A-Za-z0-9_]*"
            required
          />
        </Field>
        <Field label="Value" hint="Write-only. Never shown again.">
          <input
            type="password"
            autoComplete="new-password"
            value={value}
            onChange={(e) => {
              setValue(e.target.value);
              setSaved(null);
            }}
            required
          />
        </Field>
        <button className="btn primary" type="submit" disabled={act.busy || !project || !name.trim() || !value}>
          Save secret
        </button>
      </form>
      {saved && (
        <p className="ok-text small" role="status">
          {saved}
        </p>
      )}
      <ErrorNote error={act.error} />
    </Card>
  );
}
