// Usage & cost: runtime-reported usage, budgets and outcome metrics.
// Costs are what the runtime reports; on a subscription that is a list-price
// estimate, not money spent.

import { useMemo, useState, type FormEvent } from "react";
import { api, useAction, useQuery } from "../api";
import { useApp } from "../router";
import type { Budget, UsageBucket } from "../types";
import { Card, Empty, ErrorNote, Field, fmtNum, fmtUsd, Loading, PageHeader } from "../ui";
import { BudgetBar, Tile } from "../widgets";

type Measure = "cost" | "runs" | "tokens";
const MEASURES: { key: Measure; label: string }[] = [
  { key: "cost", label: "Reported cost (list-price estimate)" },
  { key: "runs", label: "Runs" },
  { key: "tokens", label: "Tokens (in + out)" },
];

const value = (b: UsageBucket, m: Measure) =>
  m === "cost" ? b.cost_usd : m === "runs" ? b.runs : b.input_tokens + b.output_tokens;
const fmtValue = (v: number, m: Measure) => (m === "cost" ? fmtUsd(v) : fmtNum(v));

export default function Usage() {
  const [days, setDays] = useState(30);
  const [measure, setMeasure] = useState<Measure>("cost");
  const [table, setTable] = useState(false);
  const q = useQuery(() => api.usageSummary(days), [days]);
  const u = q.data;

  return (
    <div className="page">
      <PageHeader
        title="Usage & cost"
        sub="Figures come from what the Claude Code runtime reports per run."
        actions={
          <>
            <select value={days} onChange={(e) => setDays(Number(e.target.value))} aria-label="Period">
              {[1, 7, 30, 90].map((d) => (
                <option key={d} value={d}>
                  Last {d} day{d === 1 ? "" : "s"}
                </option>
              ))}
            </select>
            <select value={measure} onChange={(e) => setMeasure(e.target.value as Measure)} aria-label="Chart measure">
              {MEASURES.map((m) => (
                <option key={m.key} value={m.key}>
                  {m.label}
                </option>
              ))}
            </select>
            <label className="check">
              <input type="checkbox" checked={table} onChange={(e) => setTable(e.target.checked)} />
              Tables
            </label>
          </>
        }
      />
      <ErrorNote error={q.error} onRetry={q.reload} />
      {!u ? (
        q.loading && <Loading />
      ) : (
        <>
          <div className="tiles wide">
            <Tile label="Reported cost" value={fmtUsd(u.reported_cost_usd)} sub="list-price estimate" />
            <Tile label="Runs" value={fmtNum(u.runs)} sub={`${u.runs_failed} failed · ${u.retries} retries`} />
            <Tile label="Tokens" value={fmtNum(u.input_tokens + u.output_tokens)} sub={`${fmtNum(u.input_tokens)} in · ${fmtNum(u.output_tokens)} out`} />
            <Tile label="Tasks done" value={fmtNum(u.tasks_done)} sub={`${u.tasks_blocked} blocked`} />
            <Tile
              label="Verification pass rate"
              value={u.verification_pass_rate == null ? "–" : `${Math.round(u.verification_pass_rate * 100)}%`}
            />
            <Tile label="Avg PR cycle" value={u.avg_pr_cycle_hours == null ? "–" : `${u.avg_pr_cycle_hours.toFixed(1)} h`} />
            <Tile label="Approvals" value={fmtNum(u.approvals)} sub={`${u.interventions} interventions`} />
            <Tile label="Hours saved (est.)" value={u.hours_saved == null ? "–" : `${u.hours_saved.toFixed(1)} h`} sub="see method below" />
          </div>
          <p className="muted small basis">
            <b>Cost basis:</b> {u.cost_basis || "reported cost (list-price estimate)"}. <b>Hours-saved method:</b>{" "}
            {u.hours_saved_method || "not reported"}.
          </p>

          <Card title={`${MEASURES.find((m) => m.key === measure)?.label} per day`}>
            {u.by_day.length === 0 ? (
              <Empty>No runs in this period.</Empty>
            ) : table ? (
              <BucketTable rows={u.by_day} keyLabel="Day" />
            ) : (
              <DayChart rows={u.by_day} measure={measure} />
            )}
          </Card>
          <div className="grid two">
            <Card title="By project">
              {table ? <BucketTable rows={u.by_project} keyLabel="Project" /> : <HBars rows={u.by_project} measure={measure} />}
            </Card>
            <Card title="By model">
              {table ? <BucketTable rows={u.by_model} keyLabel="Model" /> : <HBars rows={u.by_model} measure={measure} />}
            </Card>
          </div>
          <Budgets status={u.budgets} onChanged={q.reload} />
        </>
      )}
    </div>
  );
}

/** Vertical bars, one per day. Single series: no legend; title names it. */
function DayChart({ rows, measure }: { rows: UsageBucket[]; measure: Measure }) {
  const W = 760;
  const H = 180;
  const padL = 48;
  const padB = 22;
  const max = Math.max(...rows.map((r) => value(r, measure)), 0) || 1;
  const step = (W - padL) / rows.length;
  const bw = Math.max(2, Math.min(28, step - 2));
  const ticks = [0, 0.5, 1].map((f) => f * max);
  const labelEvery = Math.ceil(rows.length / 10);
  return (
    <svg viewBox={`0 0 ${W} ${H}`} className="chart" role="img" aria-label={`${measure} per day`}>
      {ticks.map((t) => {
        const y = (H - padB) * (1 - t / max) + 0.5;
        return (
          <g key={t}>
            <line x1={padL} x2={W} y1={y} y2={y} className="grid-line" />
            <text x={padL - 6} y={y + 4} textAnchor="end" className="axis">
              {fmtValue(t, measure)}
            </text>
          </g>
        );
      })}
      {rows.map((r, i) => {
        const v = value(r, measure);
        const h = ((H - padB) * v) / max;
        const x = padL + i * step + (step - bw) / 2;
        return (
          <g key={r.key} className="bar-g">
            <rect x={padL + i * step} y={0} width={step} height={H - padB} className="hit" />
            {h > 0 && <path d={roundTop(x, H - padB - h, bw, h)} className="bar" />}
            <title>
              {r.key}: {fmtValue(v, measure)} · {r.runs} runs
            </title>
            {i % labelEvery === 0 && (
              <text x={x + bw / 2} y={H - 6} textAnchor="middle" className="axis">
                {r.key.slice(5)}
              </text>
            )}
          </g>
        );
      })}
    </svg>
  );
}

function roundTop(x: number, y: number, w: number, h: number): string {
  const r = Math.min(4, w / 2, h);
  return `M${x},${y + h}V${y + r}Q${x},${y} ${x + r},${y}H${x + w - r}Q${x + w},${y} ${x + w},${y + r}V${y + h}Z`;
}

/** Horizontal bars with direct value labels. */
function HBars({ rows, measure }: { rows: UsageBucket[]; measure: Measure }) {
  const sorted = useMemo(() => [...rows].sort((a, b) => value(b, measure) - value(a, measure)).slice(0, 12), [rows, measure]);
  if (sorted.length === 0) return <Empty>No data.</Empty>;
  const max = value(sorted[0], measure) || 1;
  return (
    <ul className="hbars" aria-label={`${measure} breakdown`}>
      {sorted.map((r) => {
        const v = value(r, measure);
        return (
          <li key={r.key} title={`${r.key}: ${fmtValue(v, measure)} · ${r.runs} runs`}>
            <span className="hbar-key">{r.key}</span>
            <span className="hbar-track">
              <span className="hbar-fill" style={{ width: `${(v / max) * 100}%` }} />
            </span>
            <span className="hbar-val">{fmtValue(v, measure)}</span>
          </li>
        );
      })}
    </ul>
  );
}

function BucketTable({ rows, keyLabel }: { rows: UsageBucket[]; keyLabel: string }) {
  if (rows.length === 0) return <Empty>No data.</Empty>;
  return (
    <table className="dense num">
      <thead>
        <tr>
          <th>{keyLabel}</th>
          <th>Runs</th>
          <th>Input tokens</th>
          <th>Output tokens</th>
          <th>Reported cost</th>
        </tr>
      </thead>
      <tbody>
        {rows.map((r) => (
          <tr key={r.key}>
            <td>{r.key}</td>
            <td>{r.runs}</td>
            <td>{fmtNum(r.input_tokens)}</td>
            <td>{fmtNum(r.output_tokens)}</td>
            <td>{fmtUsd(r.cost_usd)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function Budgets({ status, onChanged }: { status: import("../types").BudgetStatus[]; onChanged: () => void }) {
  const { projects } = useApp();
  const q = useQuery(() => api.budgetList(), []);
  const [f, setF] = useState<Budget>({ scope: "global", period: "day", limit_usd: 10 });
  const act = useAction();
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const r = await act.run(() => api.budgetSet(f));
    if (r) {
      q.set(r);
      onChanged();
    }
  };
  const statusFor = (b: Budget) => status.find((s) => s.budget.scope === b.scope && s.budget.period === b.period);

  return (
    <Card title="Budgets" count={q.data?.length}>
      <p className="muted small">Budgets are hard stops enforced by tpd: when one is exceeded, no new runs start in that scope.</p>
      <ErrorNote error={q.error} onRetry={q.reload} />
      {q.data && q.data.length === 0 && <Empty>No budgets set.</Empty>}
      <ul className="budget-bars">
        {(q.data ?? []).map((b) => {
          const s = statusFor(b);
          return (
            <BudgetBar
              key={`${b.scope}:${b.period}`}
              name={`${b.scope} / ${b.period}`}
              spent={s?.spent_usd ?? 0}
              limit={b.limit_usd}
              exceeded={s?.exceeded ?? false}
            />
          );
        })}
      </ul>
      <form className="row" onSubmit={(e) => void submit(e)}>
        <Field label="Scope">
          <select value={f.scope} onChange={(e) => setF({ ...f, scope: e.target.value })}>
            <option value="global">global</option>
            {projects.map((p) => (
              <option key={p.slug} value={p.slug}>
                {p.slug}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Period">
          <select value={f.period} onChange={(e) => setF({ ...f, period: e.target.value })}>
            <option value="day">day</option>
            <option value="month">month</option>
          </select>
        </Field>
        <Field label="Limit (USD, reported cost)">
          <input
            type="number"
            min={0}
            step={0.5}
            value={f.limit_usd}
            onChange={(e) => setF({ ...f, limit_usd: Number(e.target.value) })}
          />
        </Field>
        <button className="btn primary" type="submit" disabled={act.busy || !(f.limit_usd >= 0)}>
          Set budget
        </button>
      </form>
      <ErrorNote error={act.error} />
    </Card>
  );
}
