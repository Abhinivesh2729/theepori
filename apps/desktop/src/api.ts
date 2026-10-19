// Typed access to tpd. Every call goes through the Tauri `rpc` command, which
// forwards to the daemon's Unix socket on a worker thread. Live updates arrive
// as `tpd-event` and are fanned out from a single Tauri listener.

import { useCallback, useEffect, useRef, useState, type Dispatch, type SetStateAction } from "react";
import { invoke, listen } from "./transport";
import type * as T from "./types";

// ------------------------------------------------------------ names

export const M = {
  // M0 (crate::method)
  STATUS: "status",
  PROJECT_LIST: "project.list",
  PROJECT_ADD: "project.add",
  AUDIT_TAIL: "audit.tail",
  AUDIT_VERIFY: "audit.verify",
  // factory::m
  PROJECT_GET: "project.get",
  PROJECT_SET_CONFIG: "project.set_config",
  TASK_LIST: "task.list",
  TASK_GET: "task.get",
  TASK_CREATE: "task.create",
  TASK_CANCEL: "task.cancel",
  TASK_RETRY: "task.retry",
  TASK_DIFF: "task.diff",
  TASK_MESSAGE: "task.message",
  TASK_OPEN_TERMINAL: "task.open_terminal",
  RUN_EVENTS: "run.events",
  RUN_STOP: "run.stop",
  PLAN_CURRENT: "plan.current",
  PLAN_GENERATE: "plan.generate",
  APPROVAL_LIST: "approval.list",
  APPROVAL_DECIDE: "approval.decide",
  SIGNAL_LIST: "signal.list",
  INTAKE_RECONCILE: "intake.reconcile",
  FACTORY_CONTROLS: "factory.controls",
  FACTORY_SET_CONTROLS: "factory.set_controls",
  FACTORY_STOP_ALL: "factory.stop_all",
  DEPS_SCAN: "deps.scan",
  DEPS_LIST: "deps.list",
  DEPLOY_LIST: "deploy.list",
  DEPLOY_RUN: "deploy.run",
  MEMORY_LIST: "memory.list",
  MEMORY_SEARCH: "memory.search",
  MEMORY_PROPOSE: "memory.propose",
  MEMORY_SET_STATUS: "memory.set_status",
  USAGE_SUMMARY: "usage.summary",
  BUDGET_LIST: "budget.list",
  BUDGET_SET: "budget.set",
  CHAT_HISTORY: "chat.history",
  CHAT_SEND: "chat.send",
  SEARCH: "search",
  VAULT_STATUS: "vault.status",
  VAULT_INIT: "vault.init",
  VAULT_UNLOCK: "vault.unlock",
  VAULT_LOCK: "vault.lock",
  VAULT_LIST: "vault.list",
  VAULT_SET: "vault.set",
  VAULT_DELETE: "vault.delete",
  EXPORT_CREATE: "export.create",
  IMPORT_PREVIEW: "import.preview",
  IMPORT_APPLY: "import.apply",
  DOCTOR: "doctor",
} as const;

export const EV = {
  // crate::event
  AUDIT_APPENDED: "audit.appended",
  PROJECT_ADDED: "project.added",
  // factory::ev
  TASK_UPDATED: "task.updated",
  RUN_EVENT: "run.event",
  RUN_UPDATED: "run.updated",
  APPROVAL_CREATED: "approval.created",
  APPROVAL_DECIDED: "approval.decided",
  PLAN_UPDATED: "plan.updated",
  CONTROLS_CHANGED: "controls.changed",
  ALERT: "alert",
  CHAT_MESSAGE: "chat.message",
} as const;

/** Payload type per event name. */
export interface EventMap {
  "audit.appended": T.AuditRecord;
  "project.added": T.Project;
  "task.updated": T.TaskSummary;
  "run.event": T.RunEvent;
  "run.updated": T.Run;
  "approval.created": T.Approval;
  "approval.decided": T.Approval;
  "plan.updated": T.Plan;
  "controls.changed": T.Controls;
  alert: T.Alert;
  "chat.message": T.ChatMessage;
}
export type EventName = keyof EventMap;
export type AnyEvent = { [K in EventName]: { event: K; data: EventMap[K] } }[EventName];

// ------------------------------------------------------------ transport

export class TpdError extends Error {}

/** Raw call. Rejects with a TpdError carrying the daemon's message. */
export async function rpc<R>(method: string, params?: object | null): Promise<R> {
  try {
    return await invoke<R>("rpc", { method, params: params ?? null });
  } catch (e) {
    throw new TpdError(typeof e === "string" ? e : e instanceof Error ? e.message : String(e));
  }
}

/** `null` when tpd is not running (uses the dedicated M0 command). */
export function daemonStatus(): Promise<T.DaemonStatus | null> {
  return invoke<T.DaemonStatus | null>("daemon_status");
}

type Id = { id: string };

export const api = {
  status: () => rpc<T.DaemonStatus>(M.STATUS),

  projectList: () => rpc<T.Project[]>(M.PROJECT_LIST),
  projectAdd: (p: T.ProjectAdd) => rpc<T.Project>(M.PROJECT_ADD, p),
  projectGet: (slug: string) => rpc<T.ProjectDetail>(M.PROJECT_GET, { slug }),
  projectSetConfig: (p: { slug: string; project_yaml?: string; verify_yaml?: string }) =>
    rpc<T.ProjectDetail>(M.PROJECT_SET_CONFIG, p),

  auditTail: (limit = 50) => rpc<T.AuditRecord[]>(M.AUDIT_TAIL, { limit } satisfies T.AuditTail),
  auditVerify: () => rpc<T.AuditVerify>(M.AUDIT_VERIFY),

  taskList: (q: T.TaskQuery = {}) => rpc<T.TaskSummary[]>(M.TASK_LIST, q),
  taskGet: (id: string) => rpc<T.TaskDetail>(M.TASK_GET, { id } satisfies Id),
  taskCreate: (t: T.TaskCreate) => rpc<T.TaskSummary>(M.TASK_CREATE, t),
  taskCancel: (id: string, reason?: string) => rpc<T.TaskSummary>(M.TASK_CANCEL, { id, reason }),
  taskRetry: (id: string) => rpc<T.TaskSummary>(M.TASK_RETRY, { id }),
  taskDiff: (id: string) => rpc<T.TaskDiff>(M.TASK_DIFF, { id }),
  taskMessage: (id: string, text: string) => rpc<T.TaskSummary>(M.TASK_MESSAGE, { id, text }),
  taskOpenTerminal: (id: string) => rpc<{ opened: boolean }>(M.TASK_OPEN_TERMINAL, { id }),

  runEvents: (run_id: string, after_seq?: number, limit?: number) =>
    rpc<T.RunEvent[]>(M.RUN_EVENTS, { run_id, after_seq, limit }),
  runStop: (run_id: string) => rpc<T.Run>(M.RUN_STOP, { run_id }),

  planCurrent: () => rpc<T.Plan | null>(M.PLAN_CURRENT),
  planGenerate: () => rpc<T.Plan>(M.PLAN_GENERATE),

  approvalList: (state?: T.ApprovalState) => rpc<T.Approval[]>(M.APPROVAL_LIST, { state }),
  approvalDecide: (d: T.ApprovalDecision) => rpc<T.Approval>(M.APPROVAL_DECIDE, d),

  signalList: (project?: string, limit?: number) => rpc<T.Signal[]>(M.SIGNAL_LIST, { project, limit }),
  intakeReconcile: (project?: string) =>
    rpc<{ signals_new: number; tasks_created: number }>(M.INTAKE_RECONCILE, { project }),

  controls: () => rpc<T.Controls>(M.FACTORY_CONTROLS),
  setControls: (u: T.ControlsUpdate) => rpc<T.Controls>(M.FACTORY_SET_CONTROLS, u),
  stopAll: () => rpc<{ stopped: number }>(M.FACTORY_STOP_ALL),

  depsScan: (project: string) => rpc<T.DepReport>(M.DEPS_SCAN, { project }),
  depsList: (project?: string) => rpc<T.DepItem[]>(M.DEPS_LIST, { project }),

  deployList: (project?: string) => rpc<T.Deployment[]>(M.DEPLOY_LIST, { project }),
  deployRun: (p: { project: string; environment: string; task_id?: string }) =>
    rpc<T.Deployment>(M.DEPLOY_RUN, p),

  memoryList: (scope?: string, status?: T.MemoryStatus) => rpc<T.MemoryItem[]>(M.MEMORY_LIST, { scope, status }),
  memorySearch: (query: string, scope?: string) => rpc<T.MemoryItem[]>(M.MEMORY_SEARCH, { query, scope }),
  memoryPropose: (p: { scope: string; category: string; text: string }) => rpc<T.MemoryItem>(M.MEMORY_PROPOSE, p),
  memorySetStatus: (id: number, status: T.MemoryStatus) => rpc<T.MemoryItem>(M.MEMORY_SET_STATUS, { id, status }),

  usageSummary: (days?: number) => rpc<T.UsageSummary>(M.USAGE_SUMMARY, { days }),
  budgetList: () => rpc<T.Budget[]>(M.BUDGET_LIST),
  budgetSet: (b: T.Budget) => rpc<T.Budget[]>(M.BUDGET_SET, b),

  chatHistory: (limit?: number) => rpc<T.ChatMessage[]>(M.CHAT_HISTORY, { limit }),
  chatSend: (text: string) => rpc<T.ChatMessage>(M.CHAT_SEND, { text }),

  search: (query: string, limit?: number) => rpc<T.SearchHit[]>(M.SEARCH, { query, limit }),

  vaultStatus: () => rpc<T.VaultStatus>(M.VAULT_STATUS),
  vaultInit: (password: string) => rpc<T.VaultStatus>(M.VAULT_INIT, { password }),
  vaultUnlock: (password: string, ttl_minutes?: number) =>
    rpc<T.VaultStatus>(M.VAULT_UNLOCK, { password, ttl_minutes }),
  vaultLock: () => rpc<T.VaultStatus>(M.VAULT_LOCK),
  vaultList: (project?: string, environment?: string) =>
    rpc<T.VaultItemMeta[]>(M.VAULT_LIST, { project, environment }),
  vaultSet: (p: { project: string; environment: string; name: string; value: string }) =>
    rpc<T.VaultItemMeta>(M.VAULT_SET, p),
  vaultDelete: (p: { project: string; environment: string; name: string }) =>
    rpc<{ deleted: boolean }>(M.VAULT_DELETE, p),

  exportCreate: (r: T.ExportRequest) => rpc<T.ExportReport>(M.EXPORT_CREATE, r),
  importPreview: (path: string) => rpc<T.ImportReport>(M.IMPORT_PREVIEW, { path }),
  importApply: (path: string) => rpc<T.ImportReport>(M.IMPORT_APPLY, { path }),

  doctor: () => rpc<T.DoctorReport>(M.DOCTOR),
};

// ------------------------------------------------------------ events

type Handler = (ev: AnyEvent) => void;
type ConnHandler = (online: boolean, reason?: string) => void;

const handlers = new Set<Handler>();
const connHandlers = new Set<ConnHandler>();
let bridged = false;

function ensureBridge() {
  if (bridged) return;
  bridged = true;
  // Listeners live for the app's lifetime; one Tauri listener per channel.
  void listen<T.TpdEvent>("tpd-event", (msg) => {
    const ev = msg.payload as AnyEvent;
    handlers.forEach((h) => {
      try {
        h(ev);
      } catch (e) {
        console.error("tpd-event handler", e);
      }
    });
  }).catch(() => undefined);
  void listen("tpd-connected", () => connHandlers.forEach((h) => h(true))).catch(() => undefined);
  void listen<string>("tpd-disconnected", (msg) =>
    connHandlers.forEach((h) => h(false, msg.payload)),
  ).catch(() => undefined);
}

/**
 * Subscribe to live tpd events. `names` limits delivery; the handler may
 * change between renders without resubscribing.
 */
export function useTpdEvents(handler: Handler, names?: readonly EventName[]) {
  const ref = useRef(handler);
  ref.current = handler;
  const key = names ? names.join(",") : "*";
  useEffect(() => {
    ensureBridge();
    const allow = key === "*" ? null : new Set(key.split(","));
    const h: Handler = (ev) => {
      if (!allow || allow.has(ev.event)) ref.current(ev);
    };
    handlers.add(h);
    return () => void handlers.delete(h);
  }, [key]);
}

/** Subscribe to tpd-connected / tpd-disconnected. */
export function useTpdConnection(handler: ConnHandler) {
  const ref = useRef(handler);
  ref.current = handler;
  useEffect(() => {
    ensureBridge();
    const h: ConnHandler = (o, r) => ref.current(o, r);
    connHandlers.add(h);
    return () => void connHandlers.delete(h);
  }, []);
}

// ------------------------------------------------------------ data hook

export interface Query<D> {
  data: D | undefined;
  error: string | null;
  loading: boolean;
  reload: () => void;
  set: Dispatch<SetStateAction<D | undefined>>;
}

/**
 * Loads `fn()` after mount (never during render), whenever `deps` change and
 * whenever tpd reconnects. Stale responses are dropped.
 */
export function useQuery<D>(fn: () => Promise<D>, deps: readonly unknown[]): Query<D> {
  const [data, setData] = useState<D | undefined>(undefined);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [tick, setTick] = useState(0);
  const fnRef = useRef(fn);
  fnRef.current = fn;
  const gen = useRef(0);

  useEffect(() => {
    const my = ++gen.current;
    setLoading(true);
    fnRef.current().then(
      (d) => {
        if (gen.current !== my) return;
        setData(d);
        setError(null);
        setLoading(false);
      },
      (e: unknown) => {
        if (gen.current !== my) return;
        setError(e instanceof Error ? e.message : String(e));
        setLoading(false);
      },
    );
  }, [tick, ...deps]);

  useTpdConnection((online) => {
    if (online) setTick((t) => t + 1);
  });

  const reload = useCallback(() => setTick((t) => t + 1), []);
  return { data, error, loading, reload, set: setData };
}

/** Runs an action, tracking busy/error state for buttons and forms. */
export function useAction() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const run = useCallback(async <R,>(fn: () => Promise<R>): Promise<R | undefined> => {
    setBusy(true);
    setError(null);
    try {
      return await fn();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      return undefined;
    } finally {
      setBusy(false);
    }
  }, []);
  return { busy, error, run, clear: () => setError(null) };
}

export function errMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
