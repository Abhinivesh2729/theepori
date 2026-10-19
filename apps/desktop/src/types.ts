// TypeScript mirrors of crates/tp-proto (lib.rs + factory.rs). Field names are
// the exact serde (snake_case) wire names; Rust enums are string unions.
// Option<T> with skip_serializing_if is `field?: T`; Option<T> without it is
// `field: T | null`.

// ---------------------------------------------------------------- lib.rs

export type ErrorCode =
  | "bad_request"
  | "unknown_method"
  | "invalid_params"
  | "not_found"
  | "conflict"
  | "internal";

export interface RpcError {
  code: ErrorCode;
  message: string;
}

/** Server-pushed notification (forwarded by the desktop as `tpd-event`). */
export interface TpdEvent<T = unknown> {
  event: string;
  data: T;
}

export interface DaemonStatus {
  protocol: number;
  version: string;
  pid: number;
  started_at: string;
  uptime_ms: number;
  factory_root: string;
  projects: number;
  audit_events: number;
  mode: Mode;
  active_runs: number;
  open_tasks: number;
  pending_approvals: number;
  vault_unlocked: boolean;
}

export interface Project {
  slug: string;
  name: string;
  repo_path: string;
  jira_key?: string;
  created_at: string;
}

export interface ProjectAdd {
  repo_path: string;
  slug?: string;
  name?: string;
  jira_key?: string;
}

export interface AuditTail {
  limit?: number;
}

export type ActorKind = "human" | "system" | "policy" | "manager" | "agent" | "steward" | "device";

export interface Actor {
  kind: ActorKind;
  id: string;
}

export type Provenance = "observed" | "claimed";
export type Outcome = "ok" | "denied" | "failed";

export interface AuditRecord {
  schema: number;
  seq: number;
  ts: string;
  actor: Actor;
  action: string;
  resource?: string;
  outcome: Outcome;
  provenance: Provenance;
  project?: string;
  task?: string;
  authorization?: string;
  details?: unknown;
  prev_hash: string;
  hash: string;
}

export interface AuditVerify {
  ok: boolean;
  records: number;
  problem?: string;
}

// ------------------------------------------------------------ factory.rs

export type Risk = "low" | "medium" | "high" | "critical";
export const RISKS: Risk[] = ["low", "medium", "high", "critical"];

export type Category = "incident" | "security" | "ci" | "bug" | "deps" | "maintenance" | "feature";
export const CATEGORIES: Category[] = [
  "incident",
  "security",
  "ci",
  "bug",
  "deps",
  "maintenance",
  "feature",
];

export type TaskState =
  | "received"
  | "triaged"
  | "planned"
  | "approved"
  | "implementing"
  | "verifying"
  | "repairing"
  | "reviewing"
  | "pr_open"
  | "awaiting_approval"
  | "merged"
  | "deploying"
  | "post_verify"
  | "done"
  | "blocked"
  | "escalated"
  | "rolled_back"
  | "cancelled";

export const TASK_STATES: TaskState[] = [
  "received",
  "triaged",
  "planned",
  "approved",
  "implementing",
  "verifying",
  "repairing",
  "reviewing",
  "pr_open",
  "awaiting_approval",
  "merged",
  "deploying",
  "post_verify",
  "done",
  "blocked",
  "escalated",
  "rolled_back",
  "cancelled",
];

export interface TaskSummary {
  id: string;
  project: string;
  title: string;
  category: Category;
  /** 1 = most urgent. */
  priority: number;
  risk: Risk;
  state: TaskState;
  source: string;
  source_url?: string;
  attempts: number;
  pr_url?: string;
  jira_key?: string;
  blocked_reason?: string;
  created_at: string;
  updated_at: string;
}

export interface TaskQuery {
  project?: string;
  /** Empty = all. */
  states?: TaskState[];
  /** Only tasks not in done/cancelled. */
  open_only?: boolean;
  limit?: number;
}

export interface TaskCreate {
  project: string;
  title: string;
  body?: string;
  category?: Category;
  priority?: number;
  /** Skip the daily plan and approve immediately (still policy-checked). */
  approve_now?: boolean;
}

export interface TaskDetail {
  task: TaskSummary;
  body: string;
  acceptance: string[];
  branch?: string;
  worktree?: string;
  runs: Run[];
  verifications: Verification[];
  findings: Finding[];
  approvals: Approval[];
  deployments: Deployment[];
}

export interface TaskDiff {
  base: string;
  files: DiffFile[];
  /** Unified diff, truncated (see `truncated`). */
  diff: string;
  truncated: boolean;
}

export interface DiffFile {
  path: string;
  additions: number;
  deletions: number;
}

export type RunKind = "implement" | "repair" | "review" | "steward" | "manager" | "intervene";
export type RunState = "starting" | "running" | "succeeded" | "failed" | "stopped" | "interrupted";

export interface Run {
  id: string;
  task_id?: string;
  kind: RunKind;
  state: RunState;
  runtime: string;
  session_id?: string;
  model?: string;
  started_at: string;
  ended_at?: string;
  cost_usd?: number;
  input_tokens?: number;
  output_tokens?: number;
  num_turns?: number;
  summary?: string;
  error?: string;
}

/** system | text | tool_use | tool_result | denied | result | stderr | tp */
export type RunEventKind =
  | "system"
  | "text"
  | "tool_use"
  | "tool_result"
  | "denied"
  | "result"
  | "stderr"
  | "tp";

export interface RunEvent {
  run_id: string;
  seq: number;
  ts: string;
  /** Usually a RunEventKind; kept open for forward compatibility. */
  kind: RunEventKind | (string & {});
  /** One-line human summary. */
  text: string;
  data?: unknown;
}

export type CheckStatus = "passed" | "failed" | "blocked" | "inconclusive" | "not_applicable";

export interface Verification {
  id: number;
  task_id: string;
  attempt: number;
  name: string;
  kind: string;
  status: CheckStatus;
  duration_ms: number;
  summary: string;
  evidence_path?: string;
  created_at: string;
}

export type FindingSeverity = "blocking" | "major" | "minor" | "info";

export interface Finding {
  id: number;
  task_id: string;
  severity: FindingSeverity | (string & {});
  file?: string;
  line?: number;
  message: string;
  /** open | resolved */
  status: "open" | "resolved" | (string & {});
}

export type ApprovalKind = "plan" | "task" | "merge" | "deploy" | "dependency" | "action" | "escalation";
export type ApprovalState = "pending" | "approved" | "rejected" | "deferred" | "expired";

export interface Approval {
  id: string;
  kind: ApprovalKind;
  state: ApprovalState;
  task_id?: string;
  project?: string;
  title: string;
  what: string;
  why: string;
  impact: string;
  risks: string;
  risk: Risk;
  reversible: boolean;
  evidence: string;
  recommendation: string;
  if_rejected: string;
  /** Escalations that skip the daily plan. */
  urgent: boolean;
  created_at: string;
  decided_at?: string;
  decided_by?: string;
  note?: string;
}

export type Decision = "approve" | "reject" | "defer";

export interface ApprovalDecision {
  id: string;
  decision: Decision;
  note?: string;
}

export type PlanState = "draft" | "approved" | "rejected" | "superseded";

export interface Plan {
  id: string;
  date: string;
  state: PlanState | (string & {});
  summary: string;
  items: PlanItem[];
  approval_id?: string;
  created_at: string;
}

export interface PlanItem {
  rank: number;
  task: TaskSummary;
  rationale: string;
}

export interface Signal {
  id: number;
  project: string;
  source: string;
  kind: string;
  external_id: string;
  title: string;
  url?: string;
  task_id?: string;
  first_seen: string;
  last_seen: string;
}

export type Mode = "normal" | "paused" | "safe";

export interface Controls {
  mode: Mode;
  auto_merge: boolean;
  auto_deploy: boolean;
  remote_access: boolean;
}

export interface ControlsUpdate {
  mode?: Mode;
  auto_merge?: boolean;
  auto_deploy?: boolean;
  remote_access?: boolean;
}

export type AlertLevel = "info" | "warning" | "critical";

export interface Alert {
  level: AlertLevel | (string & {});
  title: string;
  body: string;
  task_id?: string;
}

export interface ProjectDetail {
  project: Project;
  github_repo?: string;
  base_branch: string;
  project_yaml: string;
  verify_yaml: string;
  open_tasks: number;
}

export type Bump = "patch" | "minor" | "major" | "unknown";
export type DepState = "proposed" | "approved" | "applied" | "rejected" | "superseded";

export interface DepItem {
  id: number;
  project: string;
  ecosystem: string;
  package: string;
  from: string;
  to: string;
  bump: Bump | (string & {});
  risk: Risk;
  breaking: boolean;
  reasons: string[];
  rationale: string;
  state: DepState | (string & {});
  task_id?: string;
}

export interface DepReport {
  project: string;
  manifests: string[];
  packages: number;
  advisories: number;
  items: DepItem[];
  scanned_at: string;
  warnings?: string[];
}

export type DeploymentState = "running" | "healthy" | "failed" | "rolled_back";

export interface Deployment {
  id: string;
  project: string;
  environment: string;
  task_id?: string;
  commit: string;
  state: DeploymentState | (string & {});
  health?: string;
  version_ok?: boolean;
  rolled_back: boolean;
  authorization: string;
  started_at: string;
  finished_at?: string;
}

export type MemoryStatus = "proposed" | "validated" | "retired";

export interface MemoryItem {
  id: number;
  /** "global" or a project slug. */
  scope: string;
  category: string;
  text: string;
  status: MemoryStatus | (string & {});
  confidence: number;
  provenance: unknown;
  version: number;
  created_at: string;
  updated_at: string;
}

export interface UsageSummary {
  days: number;
  runs: number;
  runs_failed: number;
  retries: number;
  input_tokens: number;
  output_tokens: number;
  /** Sum of runtime-reported costs (list-price estimates on a subscription). */
  reported_cost_usd: number;
  cost_basis: string;
  by_project: UsageBucket[];
  by_model: UsageBucket[];
  by_day: UsageBucket[];
  tasks_done: number;
  tasks_blocked: number;
  approvals: number;
  interventions: number;
  verification_pass_rate: number | null;
  avg_pr_cycle_hours: number | null;
  hours_saved: number | null;
  hours_saved_method: string;
  budgets: BudgetStatus[];
}

export interface UsageBucket {
  key: string;
  runs: number;
  input_tokens: number;
  output_tokens: number;
  cost_usd: number;
}

export type BudgetPeriod = "day" | "month";

export interface Budget {
  /** "global" or a project slug. */
  scope: string;
  period: BudgetPeriod | (string & {});
  limit_usd: number;
}

export interface BudgetStatus {
  budget: Budget;
  spent_usd: number;
  exceeded: boolean;
}

export interface ChatMessage {
  id: number;
  role: "founder" | "manager" | (string & {});
  text: string;
  ts: string;
}

export type SearchHitKind = "project" | "task" | "approval" | "audit" | "memory" | "deployment";

export interface SearchHit {
  kind: SearchHitKind | (string & {});
  id: string;
  title: string;
  subtitle: string;
}

export interface VaultStatus {
  initialized: boolean;
  unlocked: boolean;
  lease_expires_at?: string;
  items: number;
}

export interface VaultItemMeta {
  project: string;
  environment: string;
  name: string;
  version: number;
  updated_at: string;
  expires_at?: string;
  rotation_due: boolean;
}

export interface ExportRequest {
  path: string;
  include_vault?: boolean;
  /** Explicit confirmation text required when include_vault is set. */
  vault_confirmation?: string;
  audit_days?: number;
}

export interface ExportReport {
  path: string;
  schema: number;
  included: string[];
  excluded: string[];
  reconnect: string[];
  bytes: number;
  sha256: string;
}

export interface ImportReport {
  path: string;
  schema: number;
  applied: boolean;
  projects: string[];
  memory_new: number;
  memory_duplicates: number;
  memory_conflicts: string[];
  files: string[];
  reconnect: string[];
  warnings: string[];
}

// `doctor` returns `{status, checks}` built from tp_core::doctor::Check.
export type DoctorStatus = "ok" | "warn" | "fail";

export interface DoctorCheck {
  name: string;
  status: DoctorStatus;
  detail: string;
  fix?: string;
}

export interface DoctorReport {
  status: DoctorStatus | (string & {});
  checks: DoctorCheck[];
}

// ------------------------------------------------------------ UI-only

/** Desktop-side view of the daemon connection. */
export type Link =
  | { state: "connecting" }
  | { state: "online"; status: DaemonStatus }
  | { state: "offline"; reason: string };
