// One plain-language decision request (PRD §5.3) with Approve / Reject / Defer.

import { useState } from "react";
import { api, useAction } from "./api";
import type { Approval, Decision } from "./types";
import { A, Chip, ErrorNote, fmtTime, label, relTime, RiskChip, statusTone } from "./ui";

export function ApprovalCard({
  approval: a,
  onDecided,
  compact,
}: {
  approval: Approval;
  onDecided?: (a: Approval) => void;
  compact?: boolean;
}) {
  const [note, setNote] = useState("");
  const [open, setOpen] = useState(!compact);
  const act = useAction();
  const pending = a.state === "pending";

  const decide = async (decision: Decision) => {
    const r = await act.run(() => api.approvalDecide({ id: a.id, decision, note: note.trim() || undefined }));
    if (r) {
      setNote("");
      onDecided?.(r);
    }
  };

  return (
    <article
      className={`approval ${a.urgent && pending ? "urgent" : ""} risk-${a.risk}`}
      aria-label={`${a.urgent ? "Urgent " : ""}approval: ${a.title}`}
    >
      <header className="approval-head">
        <div className="approval-title">
          {a.urgent && <Chip tone="bad">urgent</Chip>}
          <Chip tone="accent">{label(a.kind)}</Chip>
          <h3>{a.title}</h3>
        </div>
        <div className="approval-meta">
          <RiskChip risk={a.risk} />
          <Chip tone={a.reversible ? "ok" : "bad"}>{a.reversible ? "reversible" : "irreversible"}</Chip>
          {!pending && <Chip tone={statusTone(a.state)}>{a.state}</Chip>}
          {a.project && <span className="muted">{a.project}</span>}
          {a.task_id && <A to={{ view: "tasks", id: a.task_id }}>{a.task_id}</A>}
          <time className="muted" title={a.created_at}>
            {relTime(a.created_at)}
          </time>
          {compact && (
            <button className="btn small ghost" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
              {open ? "Less" : "Details"}
            </button>
          )}
        </div>
      </header>

      {!open && <p className="approval-what">{a.what}</p>}
      {open && (
        <dl className="approval-body">
          <dt>What</dt>
          <dd>{a.what}</dd>
          <dt>Why</dt>
          <dd>{a.why}</dd>
          <dt>Impact</dt>
          <dd>{a.impact}</dd>
          <dt>Risks</dt>
          <dd>{a.risks}</dd>
          <dt>Evidence</dt>
          <dd className="pre">{a.evidence || <span className="muted">none provided</span>}</dd>
          <dt>Recommendation</dt>
          <dd className="recommend">{a.recommendation}</dd>
          <dt>If rejected</dt>
          <dd>{a.if_rejected}</dd>
        </dl>
      )}

      {pending ? (
        <div className="approval-actions">
          <input
            className="note-input"
            placeholder="Optional note"
            value={note}
            onChange={(e) => setNote(e.target.value)}
            aria-label={`Note for ${a.title}`}
          />
          <button className="btn primary" disabled={act.busy} onClick={() => void decide("approve")}>
            Approve
          </button>
          <button className="btn danger" disabled={act.busy} onClick={() => void decide("reject")}>
            Reject
          </button>
          <button className="btn" disabled={act.busy} onClick={() => void decide("defer")}>
            Defer
          </button>
        </div>
      ) : (
        <p className="muted small">
          {a.state} {a.decided_by ? `by ${a.decided_by}` : ""} {a.decided_at ? `· ${fmtTime(a.decided_at)}` : ""}
          {a.note ? ` · “${a.note}”` : ""}
        </p>
      )}
      <ErrorNote error={act.error} />
    </article>
  );
}
