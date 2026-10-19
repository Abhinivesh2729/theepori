//! Factory API (M1–M10): tasks, runs, plans, approvals, verification,
//! integrations, vault, memory, usage, deployments and controls.
//!
//! Every method takes a JSON object (or null) and returns JSON. Names in
//! [`m`]; payload types below. Timestamps are RFC 3339 UTC strings.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Method names beyond the M0 set in [`crate::method`].
pub mod m {
    pub const PROJECT_GET: &str = "project.get"; // {slug} -> ProjectDetail
    pub const PROJECT_SET_CONFIG: &str = "project.set_config"; // {slug, project_yaml?, verify_yaml?} -> ProjectDetail

    pub const TASK_LIST: &str = "task.list"; // TaskQuery -> [TaskSummary]
    pub const TASK_GET: &str = "task.get"; // {id} -> TaskDetail
    pub const TASK_CREATE: &str = "task.create"; // TaskCreate -> TaskSummary
    pub const TASK_CANCEL: &str = "task.cancel"; // {id, reason?} -> TaskSummary
    pub const TASK_RETRY: &str = "task.retry"; // {id} -> TaskSummary
    pub const TASK_DIFF: &str = "task.diff"; // {id} -> TaskDiff
    pub const TASK_MESSAGE: &str = "task.message"; // {id, text} -> Run (intervene: resumes the session with the founder's message)
    pub const TASK_OPEN_TERMINAL: &str = "task.open_terminal"; // {id} -> {opened: bool}

    pub const RUN_EVENTS: &str = "run.events"; // {run_id, after_seq?, limit?} -> [RunEvent]
    pub const RUN_STOP: &str = "run.stop"; // {run_id} -> Run

    pub const PLAN_CURRENT: &str = "plan.current"; // null -> Plan | null
    pub const PLAN_GENERATE: &str = "plan.generate"; // null -> Plan

    pub const APPROVAL_LIST: &str = "approval.list"; // {state?} -> [Approval]
    pub const APPROVAL_DECIDE: &str = "approval.decide"; // ApprovalDecision -> Approval

    pub const SIGNAL_LIST: &str = "signal.list"; // {project?, limit?} -> [Signal]
    pub const INTAKE_RECONCILE: &str = "intake.reconcile"; // {project?} -> {signals_new, tasks_created}

    pub const FACTORY_CONTROLS: &str = "factory.controls"; // null -> Controls
    pub const FACTORY_SET_CONTROLS: &str = "factory.set_controls"; // ControlsUpdate -> Controls
    pub const FACTORY_STOP_ALL: &str = "factory.stop_all"; // null -> {stopped}

    pub const DEPS_SCAN: &str = "deps.scan"; // {project} -> DepReport
    pub const DEPS_LIST: &str = "deps.list"; // {project?} -> [DepItem]

    pub const DEPLOY_LIST: &str = "deploy.list"; // {project?} -> [Deployment]
    pub const DEPLOY_RUN: &str = "deploy.run"; // {project, environment, task_id?} -> Deployment

    pub const MEMORY_LIST: &str = "memory.list"; // {scope?, status?} -> [MemoryItem]
    pub const MEMORY_SEARCH: &str = "memory.search"; // {query, scope?} -> [MemoryItem]
    pub const MEMORY_PROPOSE: &str = "memory.propose"; // {scope, category, text} -> MemoryItem
    pub const MEMORY_SET_STATUS: &str = "memory.set_status"; // {id, status} -> MemoryItem

    pub const USAGE_SUMMARY: &str = "usage.summary"; // {days?} -> UsageSummary
    pub const BUDGET_LIST: &str = "budget.list"; // null -> [Budget]
    pub const BUDGET_SET: &str = "budget.set"; // Budget -> [Budget]

    pub const CHAT_HISTORY: &str = "chat.history"; // {limit?} -> [ChatMessage]
    pub const CHAT_SEND: &str = "chat.send"; // {text} -> ChatMessage (the manager's reply)

    pub const SEARCH: &str = "search"; // {query, limit?} -> [SearchHit]

    pub const VAULT_STATUS: &str = "vault.status"; // null -> VaultStatus
    pub const VAULT_INIT: &str = "vault.init"; // {password} -> VaultStatus
    pub const VAULT_UNLOCK: &str = "vault.unlock"; // {password, ttl_minutes?} -> VaultStatus
    pub const VAULT_LOCK: &str = "vault.lock"; // null -> VaultStatus
    pub const VAULT_LIST: &str = "vault.list"; // {project?, environment?} -> [VaultItemMeta]
    pub const VAULT_SET: &str = "vault.set"; // {project, environment, name, value} -> VaultItemMeta
    pub const VAULT_DELETE: &str = "vault.delete"; // {project, environment, name} -> {deleted}

    pub const EXPORT_CREATE: &str = "export.create"; // ExportRequest -> ExportReport
    pub const IMPORT_PREVIEW: &str = "import.preview"; // {path} -> ImportReport
    pub const IMPORT_APPLY: &str = "import.apply"; // {path} -> ImportReport

    pub const DOCTOR: &str = "doctor"; // null -> {status, checks}
}

/// Additional events pushed to subscribers (see also [`crate::event`]).
pub mod ev {
    pub const TASK_UPDATED: &str = "task.updated"; // TaskSummary
    pub const RUN_EVENT: &str = "run.event"; // RunEvent
    pub const RUN_UPDATED: &str = "run.updated"; // Run
    pub const APPROVAL_CREATED: &str = "approval.created"; // Approval
    pub const APPROVAL_DECIDED: &str = "approval.decided"; // Approval
    pub const PLAN_UPDATED: &str = "plan.updated"; // Plan
    pub const CONTROLS_CHANGED: &str = "controls.changed"; // Controls
    pub const ALERT: &str = "alert"; // Alert
    pub const CHAT_MESSAGE: &str = "chat.message"; // ChatMessage
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Incident,
    Security,
    Ci,
    Bug,
    Deps,
    Maintenance,
    Feature,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Received,
    Triaged,
    Planned,
    Approved,
    Implementing,
    Verifying,
    Repairing,
    Reviewing,
    PrOpen,
    AwaitingApproval,
    Merged,
    Deploying,
    PostVerify,
    Done,
    Blocked,
    Escalated,
    RolledBack,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskSummary {
    pub id: String,
    pub project: String,
    pub title: String,
    pub category: Category,
    /// 1 = most urgent.
    pub priority: u8,
    pub risk: Risk,
    pub state: TaskState,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jira_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TaskQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Filter by states; empty = all.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub states: Vec<TaskState>,
    /// Only tasks not in done/cancelled.
    #[serde(default)]
    pub open_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskCreate {
    pub project: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<Category>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u8>,
    /// Skip the daily plan and approve immediately (still policy-checked).
    #[serde(default)]
    pub approve_now: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskDetail {
    pub task: TaskSummary,
    pub body: String,
    pub acceptance: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
    pub runs: Vec<Run>,
    pub verifications: Vec<Verification>,
    pub findings: Vec<Finding>,
    pub approvals: Vec<Approval>,
    pub deployments: Vec<Deployment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskDiff {
    pub base: String,
    pub files: Vec<DiffFile>,
    /// Unified diff, truncated to `max_bytes` (see `truncated`).
    pub diff: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiffFile {
    pub path: String,
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum RunKind {
    Implement,
    Repair,
    Review,
    Steward,
    Manager,
    Intervene,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Starting,
    Running,
    Succeeded,
    Failed,
    Stopped,
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Run {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub kind: RunKind,
    pub state: RunState,
    pub runtime: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_turns: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunEvent {
    pub run_id: String,
    pub seq: u64,
    pub ts: String,
    /// system | text | tool_use | tool_result | denied | result | stderr | tp
    pub kind: String,
    /// One-line human summary (tool name + key input, text excerpt, …).
    pub text: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub data: Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Passed,
    Failed,
    Blocked,
    Inconclusive,
    NotApplicable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Verification {
    pub id: i64,
    pub task_id: String,
    pub attempt: u32,
    pub name: String,
    pub kind: String,
    pub status: CheckStatus,
    pub duration_ms: u64,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_path: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Finding {
    pub id: i64,
    pub task_id: String,
    /// blocking | major | minor | info
    pub severity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub message: String,
    /// open | resolved
    pub status: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
    Plan,
    Task,
    Merge,
    Deploy,
    Dependency,
    Action,
    Escalation,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    Pending,
    Approved,
    Rejected,
    Deferred,
    Expired,
}

/// A plain-language decision request (PRD §5.3).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Approval {
    pub id: String,
    pub kind: ApprovalKind,
    pub state: ApprovalState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub title: String,
    pub what: String,
    pub why: String,
    pub impact: String,
    pub risks: String,
    pub risk: Risk,
    pub reversible: bool,
    pub evidence: String,
    pub recommendation: String,
    pub if_rejected: String,
    /// Marks escalations that skip the daily plan (incidents, critical security).
    #[serde(default)]
    pub urgent: bool,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Approve,
    Reject,
    Defer,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApprovalDecision {
    pub id: String,
    pub decision: Decision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Plan {
    pub id: String,
    pub date: String,
    /// draft | approved | rejected | superseded
    pub state: String,
    pub summary: String,
    pub items: Vec<PlanItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlanItem {
    pub rank: u32,
    pub task: TaskSummary,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Signal {
    pub id: i64,
    pub project: String,
    pub source: String,
    pub kind: String,
    pub external_id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub first_seen: String,
    pub last_seen: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Normal operation.
    Normal,
    /// No new assignments; running work continues.
    Paused,
    /// Paused + running sessions stopped + no merges/deploys.
    Safe,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Controls {
    pub mode: Mode,
    pub auto_merge: bool,
    pub auto_deploy: bool,
    pub remote_access: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ControlsUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_merge: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_deploy: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_access: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Alert {
    /// info | warning | critical
    pub level: String,
    pub title: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectDetail {
    pub project: crate::Project,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_repo: Option<String>,
    pub base_branch: String,
    pub project_yaml: String,
    pub verify_yaml: String,
    pub open_tasks: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DepItem {
    pub id: i64,
    pub project: String,
    pub ecosystem: String,
    pub package: String,
    pub from: String,
    pub to: String,
    /// patch | minor | major | unknown
    pub bump: String,
    pub risk: Risk,
    pub breaking: bool,
    pub reasons: Vec<String>,
    pub rationale: String,
    /// proposed | approved | applied | rejected | superseded
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DepReport {
    pub project: String,
    pub manifests: Vec<String>,
    pub packages: u32,
    pub advisories: u32,
    pub items: Vec<DepItem>,
    pub scanned_at: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Deployment {
    pub id: String,
    pub project: String,
    pub environment: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub commit: String,
    /// running | healthy | failed | rolled_back
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_ok: Option<bool>,
    pub rolled_back: bool,
    pub authorization: String,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryItem {
    pub id: i64,
    /// "global" or a project slug.
    pub scope: String,
    pub category: String,
    pub text: String,
    /// proposed | validated | retired
    pub status: String,
    pub confidence: f64,
    pub provenance: Value,
    pub version: u32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageSummary {
    pub days: u32,
    pub runs: u64,
    pub runs_failed: u64,
    pub retries: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Sum of costs reported by the runtime (list-price estimates on a subscription).
    pub reported_cost_usd: f64,
    pub cost_basis: String,
    pub by_project: Vec<UsageBucket>,
    pub by_model: Vec<UsageBucket>,
    pub by_day: Vec<UsageBucket>,
    pub tasks_done: u64,
    pub tasks_blocked: u64,
    pub approvals: u64,
    pub interventions: u64,
    pub verification_pass_rate: Option<f64>,
    pub avg_pr_cycle_hours: Option<f64>,
    pub hours_saved: Option<f64>,
    pub hours_saved_method: String,
    pub budgets: Vec<BudgetStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageBucket {
    pub key: String,
    pub runs: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Budget {
    /// "global" or a project slug.
    pub scope: String,
    /// day | month
    pub period: String,
    pub limit_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BudgetStatus {
    pub budget: Budget,
    pub spent_usd: f64,
    pub exceeded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    pub id: i64,
    /// founder | manager
    pub role: String,
    pub text: String,
    pub ts: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchHit {
    /// project | task | approval | audit | memory | deployment
    pub kind: String,
    pub id: String,
    pub title: String,
    pub subtitle: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VaultStatus {
    pub initialized: bool,
    pub unlocked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_expires_at: Option<String>,
    pub items: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VaultItemMeta {
    pub project: String,
    pub environment: String,
    pub name: String,
    pub version: u32,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    pub rotation_due: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExportRequest {
    pub path: String,
    #[serde(default)]
    pub include_vault: bool,
    /// Explicit confirmation text required when include_vault is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault_confirmation: Option<String>,
    #[serde(default)]
    pub audit_days: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExportReport {
    pub path: String,
    pub schema: u32,
    pub included: Vec<String>,
    pub excluded: Vec<String>,
    pub reconnect: Vec<String>,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImportReport {
    pub path: String,
    pub schema: u32,
    pub applied: bool,
    pub projects: Vec<String>,
    pub memory_new: u32,
    pub memory_duplicates: u32,
    pub memory_conflicts: Vec<String>,
    pub files: Vec<String>,
    pub reconnect: Vec<String>,
    pub warnings: Vec<String>,
}
