#![cfg_attr(test, allow(clippy::unwrap_used))]
//! Wire types shared by `tpd`, the `tp` CLI, the desktop app and (later) the
//! relay and mobile app.
//!
//! Transport: newline-delimited JSON over the factory's Unix socket. A client
//! sends [`Request`]s and gets one [`Response`] per request id. After a
//! `subscribe` request the connection only carries [`Event`]s.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Bumped on any breaking change to methods or payloads.
pub const PROTOCOL_VERSION: u32 = 1;

pub mod method {
    pub const STATUS: &str = "status";
    pub const PROJECT_LIST: &str = "project.list";
    pub const PROJECT_ADD: &str = "project.add";
    pub const AUDIT_TAIL: &str = "audit.tail";
    pub const AUDIT_VERIFY: &str = "audit.verify";
    pub const SUBSCRIBE: &str = "subscribe";
    pub const SHUTDOWN: &str = "shutdown";
}

pub mod event {
    pub const AUDIT_APPENDED: &str = "audit.appended";
    pub const PROJECT_ADDED: &str = "project.added";
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Request {
    pub id: u64,
    pub method: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Response {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    pub fn ok(id: u64, result: Value) -> Self {
        Self { id, result: Some(result), error: None }
    }
    pub fn err(id: u64, code: ErrorCode, message: impl Into<String>) -> Self {
        Self { id, result: None, error: Some(RpcError { code, message: message.into() }) }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RpcError {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadRequest,
    UnknownMethod,
    InvalidParams,
    NotFound,
    Conflict,
    Internal,
}

/// A server-pushed notification on a subscribed connection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub event: String,
    pub data: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DaemonStatus {
    pub protocol: u32,
    pub version: String,
    pub pid: u32,
    pub started_at: String,
    pub uptime_ms: u64,
    pub factory_root: String,
    pub projects: u64,
    pub audit_events: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Project {
    pub slug: String,
    pub name: String,
    pub repo_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jira_key: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectAdd {
    pub repo_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jira_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuditTail {
    #[serde(default = "default_tail")]
    pub limit: u32,
}

fn default_tail() -> u32 {
    50
}

/// Who caused an audited action.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Actor {
    pub kind: ActorKind,
    pub id: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    Human,
    System,
    Policy,
    Manager,
    Agent,
    Steward,
    Device,
}

/// Whether tp saw something happen or an agent only said it did.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    Observed,
    Claimed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Ok,
    Denied,
    Failed,
}

/// One audit record as stored (JSONL line) and sent over the wire.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuditRecord {
    pub schema: u32,
    pub seq: u64,
    pub ts: String,
    pub actor: Actor,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    pub outcome: Outcome,
    pub provenance: Provenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization: Option<String>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub details: Value,
    pub prev_hash: String,
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuditVerify {
    pub ok: bool,
    pub records: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_round_trip_omits_null_params() {
        let r = Request { id: 7, method: method::STATUS.into(), params: Value::Null };
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(s, r#"{"id":7,"method":"status"}"#);
        assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
    }

    #[test]
    fn error_response_shape() {
        let r = Response::err(1, ErrorCode::UnknownMethod, "nope");
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            json!({"id":1,"error":{"code":"unknown_method","message":"nope"}})
        );
    }

    #[test]
    fn audit_tail_default_limit() {
        let t: AuditTail = serde_json::from_value(json!({})).unwrap();
        assert_eq!(t.limit, 50);
    }
}
