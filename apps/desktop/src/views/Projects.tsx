// Projects: registry, add, and per-project config / dependencies / deployments.

import { useEffect, useMemo, useState, type FormEvent } from "react";
import { api, useAction, useQuery } from "../api";
import { Icon } from "../icons";
import { inTauri } from "../transport";
import { href, navigate, useApp, type Route } from "../router";
import type { DepItem, DepReport, ProjectAdd, ProjectDetail } from "../types";
import {
  A,
  Card,
  Chip,
  Confirm,
  Empty,
  ErrorNote,
  Field,
  fmtTime,
  Loading,
  Modal,
  PageHeader,
  RiskChip,
  statusTone,
  Tabs,
} from "../ui";
import { DeployTable } from "../widgets";

export default function Projects({ route }: { route: Route }) {
  return route.id ? <ProjectView slug={route.id} key={route.id} /> : <ProjectList />;
}

function ProjectList() {
  const { projects, link } = useApp();
  const [adding, setAdding] = useState(false);
  return (
    <div className="page">
      <PageHeader
        title="Projects"
        sub="Repositories registered with the factory. Project config may only tighten global policy."
        actions={
          <button className="btn primary" onClick={() => setAdding(true)} disabled={link.state !== "online"}>
            Add project
          </button>
        }
      />
      {projects.length === 0 ? (
        <div className="rows">
          <button className="rowlink" onClick={() => setAdding(true)} disabled={link.state !== "online"}>
            <Icon name="plus" />
            <span className="grow">
              <span className="title">Add your first project</span>
              <span className="sub">Register a local git checkout so the factory can work on it.</span>
            </span>
          </button>
        </div>
      ) : (
        <div className="rows">
          {projects.map((p) => (
            <a key={p.slug} className="rowlink" href={href({ view: "projects", id: p.slug })}>
              <Icon name="folder" className="muted" />
              <span className="grow">
                <span className="title">
                  {p.name}
                  {p.name !== p.slug && <span className="muted mono small"> {p.slug}</span>}
                </span>
                <span className="sub mono">{p.repo_path}</span>
              </span>
              {p.jira_key && <Chip tone="accent">{p.jira_key}</Chip>}
              <span className="muted small">{fmtTime(p.created_at)}</span>
              <Icon name="chevronRight" className="go" />
            </a>
          ))}
        </div>
      )}
      {adding && <AddProject onClose={() => setAdding(false)} />}
    </div>
  );
}

function AddProject({ onClose }: { onClose: () => void }) {
  const [f, setF] = useState<ProjectAdd>({ repo_path: "" });
  const [over, setOver] = useState(false);
  const act = useAction();
  // In the app, dropping a folder from Finder fills the path.
  useEffect(() => {
    if (!inTauri) return;
    let off: (() => void) | undefined;
    let live = true;
    void import("@tauri-apps/api/webview").then(({ getCurrentWebview }) =>
      getCurrentWebview()
        .onDragDropEvent((ev) => {
          const p = ev.payload;
          if (p.type === "over" || p.type === "enter") setOver(true);
          else if (p.type === "leave") setOver(false);
          else if (p.type === "drop") {
            setOver(false);
            if (p.paths[0]) setF((x) => ({ ...x, repo_path: p.paths[0] }));
          }
        })
        .then((u) => (live ? (off = u) : u())),
    );
    return () => {
      live = false;
      off?.();
    };
  }, []);
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const clean: ProjectAdd = {
      repo_path: f.repo_path.trim(),
      slug: f.slug?.trim() || undefined,
      name: f.name?.trim() || undefined,
      jira_key: f.jira_key?.trim().toUpperCase() || undefined,
    };
    const p = await act.run(() => api.projectAdd(clean));
    if (p) {
      onClose();
      navigate({ view: "projects", id: p.slug });
    }
  };
  return (
    <Modal title="Add project" onClose={onClose}>
      <form className="form" onSubmit={(e) => void submit(e)}>
        <Field label="Repository path" hint={inTauri ? "Absolute path to a local git checkout, or drop its folder on this window." : "Absolute path to a local git checkout."}>
          <input
            value={f.repo_path}
            onChange={(e) => setF({ ...f, repo_path: e.target.value })}
            placeholder="/Users/you/code/app"
            required
            className="mono"
            style={over ? { borderColor: "var(--accent)", boxShadow: "0 0 0 3px var(--accent-soft)" } : undefined}
          />
        </Field>
        <Field label="Jira project key">
          <input value={f.jira_key ?? ""} onChange={(e) => setF({ ...f, jira_key: e.target.value })} placeholder="APP" />
        </Field>
        <div className="form-row">
          <Field label="Slug (optional)">
            <input value={f.slug ?? ""} onChange={(e) => setF({ ...f, slug: e.target.value })} />
          </Field>
          <Field label="Name (optional)">
            <input value={f.name ?? ""} onChange={(e) => setF({ ...f, name: e.target.value })} />
          </Field>
        </div>
        <ErrorNote error={act.error} />
        <div className="modal-foot">
          <button type="button" className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn primary" type="submit" disabled={!f.repo_path.trim() || act.busy}>
            {act.busy ? "Adding…" : "Add project"}
          </button>
        </div>
      </form>
    </Modal>
  );
}

type Tab = "config" | "deps" | "deploys";

function ProjectView({ slug }: { slug: string }) {
  const q = useQuery(() => api.projectGet(slug), [slug]);
  const [tab, setTab] = useState<Tab>("config");
  const { setProject, project } = useApp();

  if (q.error && !q.data) return <ErrorNote error={q.error} onRetry={q.reload} />;
  if (!q.data) return <Loading what="Loading project" />;
  const d = q.data;
  const p = d.project;

  return (
    <div className="page">
      <PageHeader
        title={p.name}
        sub={
          <span className="ws-meta">
            <span className="mono">{p.slug}</span>
            {p.jira_key && <Chip tone="accent">{p.jira_key}</Chip>}
            {d.github_repo && <span>{d.github_repo}</span>}
            <span>
              base <span className="mono">{d.base_branch}</span>
            </span>
            <A to={{ view: "tasks" }}>{d.open_tasks} open tasks</A>
          </span>
        }
        actions={
          project !== p.slug && (
            <button className="btn" onClick={() => setProject(p.slug)}>
              Scope app to {p.slug}
            </button>
          )
        }
      />
      <p className="path muted">{p.repo_path}</p>
      <Tabs
        ariaLabel="Project sections"
        value={tab}
        onChange={setTab}
        tabs={[
          { key: "config", label: "Configuration" },
          { key: "deps", label: "Dependencies" },
          { key: "deploys", label: "Deployments" },
        ]}
      />
      <section role="tabpanel" aria-label={tab}>
        {tab === "config" && <ConfigEditor d={d} onSaved={(n) => q.set(n)} />}
        {tab === "deps" && <Deps slug={slug} />}
        {tab === "deploys" && <Deploys slug={slug} />}
      </section>
    </div>
  );
}

function ConfigEditor({ d, onSaved }: { d: ProjectDetail; onSaved: (d: ProjectDetail) => void }) {
  const [proj, setProj] = useState(d.project_yaml);
  const [verify, setVerify] = useState(d.verify_yaml);
  const act = useAction();
  const [saved, setSaved] = useState(false);
  const dirty = proj !== d.project_yaml || verify !== d.verify_yaml;

  const save = async () => {
    setSaved(false);
    const r = await act.run(() =>
      api.projectSetConfig({
        slug: d.project.slug,
        project_yaml: proj !== d.project_yaml ? proj : undefined,
        verify_yaml: verify !== d.verify_yaml ? verify : undefined,
      }),
    );
    if (r) {
      onSaved(r);
      setProj(r.project_yaml);
      setVerify(r.verify_yaml);
      setSaved(true);
    }
  };

  return (
    <div className="config">
      <div className="config-grid">
        <Field label="project.yaml">
          <textarea className="code" spellCheck={false} rows={22} value={proj} onChange={(e) => setProj(e.target.value)} />
        </Field>
        <Field label="verify.yaml">
          <textarea className="code" spellCheck={false} rows={22} value={verify} onChange={(e) => setVerify(e.target.value)} />
        </Field>
      </div>
      <ErrorNote error={act.error} />
      <div className="row">
        <button className="btn primary" disabled={!dirty || act.busy} onClick={() => void save()}>
          {act.busy ? "Saving…" : "Save configuration"}
        </button>
        <button
          className="btn"
          disabled={!dirty || act.busy}
          onClick={() => {
            setProj(d.project_yaml);
            setVerify(d.verify_yaml);
          }}
        >
          Revert
        </button>
        {saved && !dirty && <span className="ok-text small">Saved. tpd validated and audited the change.</span>}
        {dirty && <span className="muted small">Unsaved changes</span>}
      </div>
    </div>
  );
}

function Deps({ slug }: { slug: string }) {
  const q = useQuery(() => api.depsList(slug), [slug]);
  const [report, setReport] = useState<DepReport | null>(null);
  const act = useAction();
  const [showAll, setShowAll] = useState(false);

  const scan = async () => {
    const r = await act.run(() => api.depsScan(slug));
    if (r) {
      setReport(r);
      q.set(r.items);
    }
  };
  const items = useMemo(
    () =>
      (q.data ?? []).filter((i) => showAll || i.state === "proposed" || i.state === "approved"),
    [q.data, showAll],
  );

  return (
    <Card
      title="Dependencies"
      count={items.length}
      actions={
        <>
          <label className="check">
            <input type="checkbox" checked={showAll} onChange={(e) => setShowAll(e.target.checked)} />
            Show applied / rejected
          </label>
          <button className="btn primary" onClick={() => void scan()} disabled={act.busy}>
            {act.busy ? "Scanning…" : "Scan now"}
          </button>
        </>
      }
    >
      <ErrorNote error={act.error ?? q.error} />
      {report && (
        <div className="note info">
          Scanned {report.manifests.length} manifests · {report.packages} packages · {report.advisories} advisories ·{" "}
          {fmtTime(report.scanned_at)}
          {report.manifests.length > 0 && report.packages === 0 && (
            <p className="small">
              No resolved versions found. Versions come from lockfiles: commit <code>package-lock.json</code>, <code>Cargo.lock</code>,{" "}
              <code>poetry.lock</code> or pinned <code>requirements.txt</code> (for npm: <code>npm install --package-lock-only</code>).
            </p>
          )}
          {report.warnings && report.warnings.length > 0 && (
            <ul>
              {report.warnings.map((w, i) => (
                <li key={i}>{w}</li>
              ))}
            </ul>
          )}
        </div>
      )}
      {q.loading && !q.data ? (
        <Loading />
      ) : items.length === 0 ? (
        <Empty>No dependency updates proposed.</Empty>
      ) : (
        <table className="dense">
          <thead>
            <tr>
              <th>Package</th>
              <th>Update</th>
              <th>Bump</th>
              <th>Risk</th>
              <th>State</th>
              <th>Rationale</th>
            </tr>
          </thead>
          <tbody>
            {items.map((i: DepItem) => (
              <tr key={i.id}>
                <td>
                  <b>{i.package}</b> <span className="muted small">{i.ecosystem}</span>
                </td>
                <td className="mono small">
                  {i.from} → {i.to}
                </td>
                <td>
                  <Chip tone={i.bump === "major" ? "warn" : "muted"}>{i.bump}</Chip>
                  {i.breaking && <Chip tone="bad">breaking</Chip>}
                </td>
                <td>
                  <RiskChip risk={i.risk} />
                </td>
                <td>
                  <Chip tone={statusTone(i.state)}>{i.state}</Chip>
                  {i.task_id && (
                    <>
                      {" "}
                      <A to={{ view: "tasks", id: i.task_id }}>{i.task_id}</A>
                    </>
                  )}
                </td>
                <td>
                  {i.rationale}
                  {i.reasons.length > 0 && <div className="muted small">{i.reasons.join(" · ")}</div>}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </Card>
  );
}

function Deploys({ slug }: { slug: string }) {
  const q = useQuery(() => api.deployList(slug), [slug]);
  const [env, setEnv] = useState("");
  const [taskId, setTaskId] = useState("");
  const [confirm, setConfirm] = useState(false);
  const act = useAction();
  const envs = useMemo(() => [...new Set((q.data ?? []).map((d) => d.environment))], [q.data]);
  const rows = useMemo(() => [...(q.data ?? [])].sort((a, b) => b.started_at.localeCompare(a.started_at)), [q.data]);

  const run = async () => {
    const r = await act.run(() =>
      api.deployRun({ project: slug, environment: env.trim(), task_id: taskId.trim() || undefined }),
    );
    if (r) {
      setConfirm(false);
      q.set((prev) => [r, ...(prev ?? [])]);
    }
  };

  return (
    <Card title="Deployments" count={rows.length}>
      <form
        className="row"
        onSubmit={(e) => {
          e.preventDefault();
          if (env.trim()) setConfirm(true);
        }}
      >
        <Field label="Environment">
          <input value={env} onChange={(e) => setEnv(e.target.value)} list="deploy-envs" placeholder="staging" required />
        </Field>
        <datalist id="deploy-envs">
          {envs.map((e) => (
            <option key={e} value={e} />
          ))}
        </datalist>
        <Field label="Task (optional)">
          <input value={taskId} onChange={(e) => setTaskId(e.target.value)} className="mono" />
        </Field>
        <button className="btn primary" type="submit" disabled={!env.trim()}>
          Deploy…
        </button>
      </form>
      <ErrorNote error={q.error} onRetry={q.reload} />
      {q.loading && !q.data ? <Loading /> : rows.length === 0 ? <Empty>No deployments yet.</Empty> : <DeployTable rows={rows} />}
      {confirm && (
        <Confirm
          title={`Deploy ${slug} to ${env.trim()}?`}
          body={
            <p>
              Runs the project's deploy adapter, then health and version checks. A failed health check rolls back automatically.
              Policy may still require an approval; if so it appears in the Approval Center.
            </p>
          }
          confirmLabel="Deploy"
          danger={/prod/i.test(env)}
          typed={/prod/i.test(env) ? env.trim() : undefined}
          busy={act.busy}
          error={act.error}
          onClose={() => setConfirm(false)}
          onConfirm={() => void run()}
        />
      )}
    </Card>
  );
}
