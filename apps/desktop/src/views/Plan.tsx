// Daily Plan: one approval authorizes every item within its risk class.

import { useState } from "react";
import { api, useAction, useQuery, useTpdEvents } from "../api";
import type { Decision } from "../types";
import { A, Card, Chip, Empty, ErrorNote, fmtTime, label, Loading, PageHeader, RiskChip, StateChip, statusTone } from "../ui";

export default function PlanView() {
  const plan = useQuery(() => api.planCurrent(), []);
  const gen = useAction();
  const decide = useAction();
  const [note, setNote] = useState("");

  useTpdEvents((ev) => {
    if (ev.event === "plan.updated") plan.set(ev.data);
    if (ev.event === "approval.decided" && plan.data?.approval_id === ev.data.id) plan.reload();
  }, ["plan.updated", "approval.decided"]);

  const p = plan.data;
  const generate = async () => {
    const r = await gen.run(() => api.planGenerate());
    if (r) plan.set(r);
  };
  const onDecide = async (decision: Decision) => {
    if (!p?.approval_id) return;
    const id = p.approval_id;
    const r = await decide.run(() => api.approvalDecide({ id, decision, note: note.trim() || undefined }));
    if (r) {
      setNote("");
      plan.reload();
    }
  };

  return (
    <div className="page">
      <PageHeader
        title="Daily Plan"
        sub="Signals collect all day; one plan is drafted and one approval authorizes its items. Critical incidents skip the plan."
        actions={
          <button className="btn primary" onClick={() => void generate()} disabled={gen.busy}>
            {gen.busy ? "Generating…" : p ? "Regenerate plan" : "Generate plan"}
          </button>
        }
      />
      <ErrorNote error={gen.error} />
      <ErrorNote error={plan.error} onRetry={plan.reload} />
      {plan.loading && !p ? (
        <Loading />
      ) : !p ? (
        <Empty>No plan for today yet. Generate one to rank today's work.</Empty>
      ) : (
        <>
          <Card
            title={`Plan for ${p.date}`}
            actions={
              <>
                <Chip tone={statusTone(p.state)}>{p.state}</Chip>
                <span className="muted small">created {fmtTime(p.created_at)}</span>
              </>
            }
          >
            <p className="plan-summary">{p.summary}</p>
            {p.state === "draft" && p.approval_id && (
              <div className="approval-actions">
                <input
                  className="note-input"
                  value={note}
                  onChange={(e) => setNote(e.target.value)}
                  placeholder="Optional note"
                  aria-label="Note for plan decision"
                />
                <button className="btn primary" disabled={decide.busy} onClick={() => void onDecide("approve")}>
                  Approve plan
                </button>
                <button className="btn danger" disabled={decide.busy} onClick={() => void onDecide("reject")}>
                  Reject
                </button>
                <button className="btn" disabled={decide.busy} onClick={() => void onDecide("defer")}>
                  Defer
                </button>
                <A to={{ view: "approvals", id: p.approval_id }} className="small">
                  View approval
                </A>
              </div>
            )}
            <ErrorNote error={decide.error} />
          </Card>

          <Card title="Ranked items" count={p.items.length}>
            {p.items.length === 0 ? (
              <Empty>The plan has no items.</Empty>
            ) : (
              <ol className="plan-items">
                {[...p.items]
                  .sort((a, b) => a.rank - b.rank)
                  .map((i) => (
                    <li key={i.task.id} className="plan-item">
                      <span className="rank" aria-label={`Rank ${i.rank}`}>
                        {i.rank}
                      </span>
                      <div className="plan-item-body">
                        <div className="plan-item-head">
                          <A to={{ view: "tasks", id: i.task.id }}>{i.task.title}</A>
                          <span className="muted">{i.task.project}</span>
                          <Chip>{label(i.task.category)}</Chip>
                          <Chip tone="muted">P{i.task.priority}</Chip>
                          <RiskChip risk={i.task.risk} />
                          <StateChip state={i.task.state} />
                        </div>
                        <p className="rationale">{i.rationale}</p>
                      </div>
                    </li>
                  ))}
              </ol>
            )}
          </Card>
        </>
      )}
    </div>
  );
}
