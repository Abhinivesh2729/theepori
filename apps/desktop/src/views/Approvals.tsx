// Approval Center: pending decisions (urgent first) and decision history.

import { useEffect, useMemo, useState } from "react";
import { api, useQuery, useTpdEvents } from "../api";
import { ApprovalCard } from "../ApprovalCard";
import { useApp, type Route } from "../router";
import type { Approval, ApprovalKind } from "../types";
import { Empty, ErrorNote, label, Loading, PageHeader, Tabs } from "../ui";

type Tab = "pending" | "history";
const HISTORY_LIMIT = 200;

export default function Approvals({ route }: { route: Route }) {
  const { project } = useApp();
  const [tab, setTab] = useState<Tab>("pending");
  const [kind, setKind] = useState<ApprovalKind | "">("");
  const pending = useQuery(() => api.approvalList("pending"), []);
  const history = useQuery<Approval[] | null>(
    () => (tab === "history" ? api.approvalList() : Promise.resolve(null)),
    [tab],
  );

  useTpdEvents((ev) => {
    if (ev.event === "approval.created") {
      pending.set((prev) => (prev && !prev.some((a) => a.id === ev.data.id) ? [ev.data, ...prev] : prev));
    } else if (ev.event === "approval.decided") {
      const a = ev.data;
      pending.set((prev) => prev?.filter((x) => x.id !== a.id));
      history.set((prev) => (prev ? [a, ...prev.filter((x) => x.id !== a.id)] : prev));
    }
  }, ["approval.created", "approval.decided"]);

  // Deep link from search: #/approvals/<id>
  useEffect(() => {
    if (!route.id) return;
    const el = document.getElementById(`approval-${route.id}`);
    el?.scrollIntoView({ block: "center" });
    el?.querySelector<HTMLElement>("button, input")?.focus();
  }, [route.id, pending.data]);

  const filter = (list: Approval[]) =>
    list.filter((a) => (!project || a.project === project) && (!kind || a.kind === kind));

  const pend = useMemo(
    () =>
      filter(pending.data ?? []).sort(
        (a, b) => Number(b.urgent) - Number(a.urgent) || a.created_at.localeCompare(b.created_at),
      ),
    [pending.data, project, kind],
  );
  const hist = useMemo(
    () =>
      filter((history.data ?? []).filter((a) => a.state !== "pending"))
        .sort((a, b) => (b.decided_at ?? b.created_at).localeCompare(a.decided_at ?? a.created_at))
        .slice(0, HISTORY_LIMIT),
    [history.data, project, kind],
  );

  const kinds: ApprovalKind[] = ["plan", "task", "merge", "deploy", "dependency", "action", "escalation"];
  const urgentCount = pend.filter((a) => a.urgent).length;

  return (
    <div className="page">
      <PageHeader
        title="Approval Center"
        sub={
          <>
            Every consequential action waits here. {urgentCount > 0 && <b className="bad-text">{urgentCount} urgent.</b>}
            {project && <> Scoped to <b>{project}</b>.</>}
          </>
        }
        actions={
          <select value={kind} onChange={(e) => setKind(e.target.value as ApprovalKind | "")} aria-label="Filter by kind">
            <option value="">All kinds</option>
            {kinds.map((k) => (
              <option key={k} value={k}>
                {label(k)}
              </option>
            ))}
          </select>
        }
      />
      <Tabs
        ariaLabel="Approvals"
        value={tab}
        onChange={setTab}
        tabs={[
          { key: "pending", label: `Pending${pending.data ? ` (${pend.length})` : ""}` },
          { key: "history", label: "History" },
        ]}
      />
      {tab === "pending" ? (
        <section role="tabpanel" aria-label="Pending approvals">
          <ErrorNote error={pending.error} onRetry={pending.reload} />
          {pending.loading && !pending.data ? (
            <Loading />
          ) : pend.length === 0 ? (
            <Empty>Nothing waiting on you.</Empty>
          ) : (
            <div className="stack">
              {pend.map((a) => (
                <div key={a.id} id={`approval-${a.id}`} className={route.id === a.id ? "highlight" : ""}>
                  <ApprovalCard approval={a} onDecided={(d) => pending.set((prev) => prev?.filter((x) => x.id !== d.id))} />
                </div>
              ))}
            </div>
          )}
        </section>
      ) : (
        <section role="tabpanel" aria-label="Approval history">
          <ErrorNote error={history.error} onRetry={history.reload} />
          {history.loading && !history.data ? (
            <Loading />
          ) : hist.length === 0 ? (
            <Empty>No decisions yet.</Empty>
          ) : (
            <div className="stack">
              {hist.map((a) => (
                <div key={a.id} id={`approval-${a.id}`}>
                  <ApprovalCard approval={a} compact />
                </div>
              ))}
            </div>
          )}
        </section>
      )}
    </div>
  );
}
