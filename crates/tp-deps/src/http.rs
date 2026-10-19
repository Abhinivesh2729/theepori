//! Shared blocking HTTP helpers (ureq).

use std::time::Duration;

use serde::Serialize;

use crate::{Error, Result};

pub(crate) const USER_AGENT: &str = concat!(
    "tp-deps/",
    env!("CARGO_PKG_VERSION"),
    " (Thee Pori software factory; https://github.com/Abhinivesh2729/theepori)"
);

/// Largest response body accepted (crates.io crate documents can be large).
const BODY_LIMIT: u64 = 32 * 1024 * 1024;

pub(crate) fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .timeout_connect(Some(timeout.min(Duration::from_secs(10))))
        .user_agent(USER_AGENT)
        .http_status_as_error(true)
        .build()
        .into()
}

/// GET a JSON document. `Ok(None)` on 404.
pub(crate) fn get_json(agent: &ureq::Agent, url: &str) -> Result<Option<serde_json::Value>> {
    match agent.get(url).header("Accept", "application/json").call() {
        Ok(mut resp) => resp
            .body_mut()
            .with_config()
            .limit(BODY_LIMIT)
            .read_json()
            .map(Some)
            .map_err(|e| http_error(url, e)),
        Err(ureq::Error::StatusCode(404)) => Ok(None),
        Err(e) => Err(http_error(url, e)),
    }
}

/// POST a JSON body and read a JSON response.
pub(crate) fn post_json<B: Serialize>(
    agent: &ureq::Agent,
    url: &str,
    body: &B,
) -> Result<serde_json::Value> {
    let mut resp = agent
        .post(url)
        .header("Accept", "application/json")
        .send_json(body)
        .map_err(|e| http_error(url, e))?;
    resp.body_mut()
        .with_config()
        .limit(BODY_LIMIT)
        .read_json()
        .map_err(|e| http_error(url, e))
}

fn http_error(url: &str, e: ureq::Error) -> Error {
    let message = match &e {
        ureq::Error::StatusCode(429) => "rate limited (HTTP 429); wait and retry".to_string(),
        ureq::Error::StatusCode(s) if *s >= 500 => {
            format!("server error (HTTP {s}); the service may be down, retry later")
        }
        ureq::Error::Timeout(_) => {
            "timed out; check network connectivity or raise the timeout".to_string()
        }
        ureq::Error::HostNotFound | ureq::Error::ConnectionFailed => {
            format!("{e}; check network connectivity and proxy settings")
        }
        _ => e.to_string(),
    };
    Error::Http {
        url: url.to_string(),
        message,
    }
}
