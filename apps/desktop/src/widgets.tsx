// Widgets shared by several views (kept out of view chunks).

import type { AuditRecord, Deployment } from "./types";
import { A, Chip, Empty, fmtTime, fmtUsd, label, statusTone } from "./ui";

export function Tile({ label, value, sub, tone }: { label: string; value: string; sub?: string; tone?: string }) {
  return (
    <div className={`tile ${tone ?? ""}`}>
      <span className="tile-label">{label}</span>
      <span className="tile-value">{value}</span>
      {sub && <span className="tile-sub">{sub}</span>}
    </div>
  );
}

export function BudgetBar({ name, spent, limit, exceeded }: { name: string; spent: number; limit: number; exceeded: boolean }) {
  const pct = limit > 0 ? Math.min(100, (spent / limit) * 100) : 0;
  return (
    <li className="budget">
      <span className="budget-name">{name}</span>
      <span
        className="meter"
        role="meter"
        aria-valuemin={0}
        aria-valuemax={limit}
        aria-valuenow={spent}
        aria-label={`${name} budget`}
      >
        <span className={`meter-fill ${exceeded ? "bad" : pct > 80 ? "warn" : ""}`} style={{ width: `${pct}%` }} />
      </span>
      <span className={exceeded ? "bad-text" : "muted"}>
        {fmtUsd(spent)} / {fmtUsd(limit)}
        {exceeded ? " · exceeded" : ""}
      </span>
    </li>
  );
}

export function AuditList({ records }: { records: AuditRecord[] }) {
  if (records.length === 0) return <Empty>No audit records.</Empty>;
  return (
    <ol className="audit">
      {records.map((r) => (
        <AuditRow key={r.seq} r={r} />
      ))}
    </ol>
  );
}

export function AuditRow({ r, as: Tag = "li" }: { r: AuditRecord; as?: "li" | "div" }) {
  return (
    <Tag className={`audit-row ${r.outcome}`}>
      <span className="seq">#{r.seq}</span>
      <time>{fmtTime(r.ts)}</time>
      <span className="action">{r.action}</span>
      <span className="actor">
        {r.actor.kind}:{r.actor.id}
      </span>
      {r.resource && <span className="resource">{r.resource}</span>}
      {r.project && <span className="project">{r.project}</span>}
      {r.task && <A to={{ view: "tasks", id: r.task }}>{r.task}</A>}
      {r.outcome !== "ok" && <Chip tone="bad">{r.outcome}</Chip>}
      {r.provenance === "claimed" && <span className="claimed">claimed</span>}
    </Tag>
  );
}

export function DeployTable({ rows }: { rows: Deployment[] }) {
  return (
    <table className="dense">
      <thead>
        <tr>
          <th>Environment</th>
          <th>State</th>
          <th>Commit</th>
          <th>Health</th>
          <th>Version</th>
          <th>Authorized by</th>
          <th>Started</th>
          <th>Finished</th>
        </tr>
      </thead>
      <tbody>
        {rows.map((x) => (
          <tr key={x.id}>
            <td>{x.environment}</td>
            <td>
              <Chip tone={statusTone(x.state)}>{label(x.state)}</Chip>
              {x.rolled_back && x.state !== "rolled_back" && <Chip tone="bad">rolled back</Chip>}
            </td>
            <td className="mono">{x.commit.slice(0, 10)}</td>
            <td>{x.health ?? "–"}</td>
            <td>{x.version_ok == null ? "–" : x.version_ok ? "ok" : <span className="bad-text">mismatch</span>}</td>
            <td className="mono small">{x.authorization}</td>
            <td className="muted">{fmtTime(x.started_at)}</td>
            <td className="muted">{fmtTime(x.finished_at)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
