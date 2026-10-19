// Mirrors crates/tp-proto. Hand-written for M0; generated from tp-proto once
// the protocol is frozen (M2).

export interface DaemonStatus {
  protocol: number;
  version: string;
  pid: number;
  started_at: string;
  uptime_ms: number;
  factory_root: string;
  projects: number;
  audit_events: number;
}

export interface Project {
  slug: string;
  name: string;
  repo_path: string;
  jira_key?: string;
  created_at: string;
}

export interface AuditRecord {
  schema: number;
  seq: number;
  ts: string;
  actor: { kind: string; id: string };
  action: string;
  resource?: string;
  outcome: "ok" | "denied" | "failed";
  provenance: "observed" | "claimed";
  project?: string;
  task?: string;
  authorization?: string;
  details?: unknown;
  prev_hash: string;
  hash: string;
}

export interface TpdEvent {
  event: string;
  data: unknown;
}

/** Desktop-side view of the daemon connection. */
export type Link =
  | { state: "connecting" }
  | { state: "online"; status: DaemonStatus }
  | { state: "offline"; reason: string };
