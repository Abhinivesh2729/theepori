#![cfg_attr(test, allow(clippy::unwrap_used))]
//! Factory core: everything `tpd`, the `tp` CLI and the desktop app share.

pub mod analytics;
pub mod audit;
pub mod client;
pub mod config;
pub mod db;
pub mod deploy;
pub mod doctor;
pub mod engine;
pub mod git;
pub mod github;
pub mod hook;
pub mod jira;
pub mod launchd;
pub mod manager;
pub mod mcp;
pub mod memory;
pub mod paths;
pub mod policy;
pub mod portability;
pub mod project_config;
pub mod redact;
pub mod registry;
pub mod runtime;
pub mod setup;
pub mod store;
pub mod verify;

mod error;
pub use error::{Error, Result};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Current UTC time as RFC 3339 with millisecond precision.
pub fn now_rfc3339() -> String {
    let now = time::OffsetDateTime::now_utc();
    let fmt = time::macros::format_description!(
        "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z"
    );
    now.format(&fmt)
        .unwrap_or_else(|_| now.unix_timestamp().to_string())
}
