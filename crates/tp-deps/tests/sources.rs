#![allow(clippy::unwrap_used)]
//! OSV and registry clients against a loopback stub server (no external
//! network), plus fixture-based OSV document parsing and one ignored live test.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{Value, json};
use tp_deps::{
    AdvisorySource, BATCH_LIMIT, Ecosystem, Error, Osv, Package, Registries, RegistrySource,
    Severity, advisory_from_vuln,
};

fn osv_fixture(id: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/osv/{id}.json"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn pkg(eco: Ecosystem, name: &str, version: &str) -> Package {
    Package {
        name: name.into(),
        version: version.into(),
        ecosystem: eco,
        direct: true,
    }
}

// ------------------------------------------------------------ stub server

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    body: String,
    user_agent: String,
}

type Handler = dyn Fn(&str, &str, &str) -> (u16, String) + Send + Sync;

/// Minimal HTTP/1.1 server: one request per connection, JSON responses.
fn serve(handler: Box<Handler>) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let handler: Arc<Handler> = Arc::from(handler);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).is_err() {
                continue;
            }
            let mut parts = request_line.split_whitespace();
            let method = parts.next().unwrap_or("").to_string();
            let path = parts.next().unwrap_or("").to_string();
            let mut len = 0usize;
            let mut user_agent = String::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                let lower = line.to_ascii_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
                if lower.starts_with("user-agent:") {
                    user_agent = line["user-agent:".len()..].trim().to_string();
                }
            }
            let mut body = vec![0u8; len];
            reader.read_exact(&mut body).unwrap();
            let body = String::from_utf8_lossy(&body).into_owned();
            let (status, resp) = handler(&method, &path, &body);
            log.lock().unwrap().push(Seen {
                method,
                path,
                body,
                user_agent,
            });
            let reason = if status == 200 { "OK" } else { "Error" };
            let out = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp}",
                resp.len()
            );
            let _ = stream.write_all(out.as_bytes());
        }
    });
    (format!("http://{addr}"), seen)
}

// ------------------------------------------------------------ OSV parsing

#[test]
fn osv_document_to_advisory() {
    let vuln = osv_fixture("GHSA-35jh-r3h4-6jhm");
    let a = advisory_from_vuln(&vuln, &pkg(Ecosystem::Npm, "lodash", "4.17.20"));
    assert_eq!(a.id, "GHSA-35jh-r3h4-6jhm");
    assert_eq!(a.aliases, vec!["CVE-2021-23337"]);
    assert_eq!(a.summary, "Command Injection in lodash");
    assert_eq!(a.severity, Severity::High); // CVSS 7.2 and label HIGH
    assert_eq!(a.fixed_versions, vec!["4.17.21"]); // lodash-es range ignored
    assert_eq!(a.affected_version, "4.17.20");
}

#[test]
fn osv_multi_range_details_summary_and_normalized_name() {
    let vuln = osv_fixture("PYSEC-2021-0001");
    let a = advisory_from_vuln(&vuln, &pkg(Ecosystem::PyPI, "django", "3.2.0"));
    // GIT range commit hashes are not versions.
    assert_eq!(a.fixed_versions, vec!["2.2.21", "3.1.10", "3.2.2"]);
    assert_eq!(a.severity, Severity::High); // computed CVSS 7.5
    assert_eq!(
        a.summary,
        "Django 3.2 before 3.2.2 and 3.1 before 3.1.10 allow path traversal in uploads."
    );
}

#[test]
fn osv_without_severity_is_unknown() {
    let vuln = osv_fixture("RUSTSEC-2020-0001");
    let a = advisory_from_vuln(&vuln, &pkg(Ecosystem::Cargo, "smallvec", "1.6.0"));
    assert_eq!(a.severity, Severity::Unknown);
    assert_eq!(a.fixed_versions, vec!["0.6.14", "1.6.1"]);
}

#[test]
fn osv_numeric_scores_and_affected_labels() {
    let vuln = json!({
        "id": "X-1",
        "severity": [{"type": "CVSS_V4", "score": "CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:H/VI:H/VA:H/SC:N/SI:N/SA:N"}],
        "affected": [{
            "package": {"ecosystem": "Pub", "name": "http"},
            "database_specific": {"severity": "MODERATE"},
            "severity": [{"type": "CVSS_V3", "score": "4.3"}],
            "ranges": [{"type": "ECOSYSTEM", "events": [{"introduced": "0"}, {"fixed": "1.0.0"}]}]
        }]
    });
    let a = advisory_from_vuln(&vuln, &pkg(Ecosystem::Pub, "http", "0.13.5"));
    assert_eq!(a.severity, Severity::Medium);
    assert_eq!(a.fixed_versions, vec!["1.0.0"]);
}

// ------------------------------------------------------------ OSV client

#[test]
fn osv_client_batches_paginates_and_fetches_details() {
    let ghsa = osv_fixture("GHSA-35jh-r3h4-6jhm").to_string();
    let (base, seen) = serve(Box::new(move |method, path, body| match (method, path) {
        ("POST", "/v1/querybatch") => {
            let req: Value = serde_json::from_str(body).unwrap();
            let results: Vec<Value> = req["queries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|q| {
                    let name = q["package"]["name"].as_str().unwrap();
                    match (name, q.get("page_token").and_then(Value::as_str)) {
                        ("lodash", None) => json!({
                            "vulns": [{"id": "GHSA-35jh-r3h4-6jhm", "modified": "x"}],
                            "next_page_token": "page2"
                        }),
                        ("lodash", Some("page2")) => {
                            json!({"vulns": [{"id": "GHSA-gone", "modified": "x"}]})
                        }
                        _ => json!({}),
                    }
                })
                .collect();
            (200, json!({ "results": results }).to_string())
        }
        ("GET", "/v1/vulns/GHSA-35jh-r3h4-6jhm") => (200, ghsa.clone()),
        ("GET", _) => (404, "{}".into()),
        _ => (400, "{}".into()),
    }));

    // BATCH_LIMIT + 1 packages force two batches.
    let mut pkgs: Vec<Package> = (0..BATCH_LIMIT)
        .map(|i| pkg(Ecosystem::Npm, &format!("filler-{i}"), "1.0.0"))
        .collect();
    pkgs.push(pkg(Ecosystem::Npm, "lodash", "4.17.20"));
    pkgs.push(pkg(Ecosystem::Npm, "lodash", "4.17.20")); // duplicate is queried once

    let advisories = Osv::new(&base).query(&pkgs).unwrap();
    let ids: Vec<&str> = advisories.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, vec!["GHSA-35jh-r3h4-6jhm", "GHSA-gone"]);
    assert_eq!(advisories[0].severity, Severity::High);
    assert_eq!(advisories[0].fixed_versions, vec!["4.17.21"]);
    assert_eq!(advisories[1].summary, "details unavailable from OSV");
    assert!(advisories.iter().all(|a| a.package == "lodash"));

    let seen = seen.lock().unwrap();
    let batches: Vec<usize> = seen
        .iter()
        .filter(|s| s.method == "POST" && s.path == "/v1/querybatch")
        .map(|s| {
            let v: Value = serde_json::from_str(&s.body).unwrap();
            v["queries"].as_array().unwrap().len()
        })
        .collect();
    assert_eq!(batches, vec![BATCH_LIMIT, 1, 1], "two batches + one page");
    let first: Value = serde_json::from_str(&seen[0].body).unwrap();
    assert_eq!(first["queries"][0]["package"]["ecosystem"], "npm");
    assert_eq!(first["queries"][0]["version"], "1.0.0");
    assert!(seen.iter().all(|s| s.user_agent.starts_with("tp-deps/")));
}

#[test]
fn osv_client_reports_http_failures_with_a_remedy() {
    let (base, _) = serve(Box::new(|_, _, _| (503, "{}".into())));
    let err = Osv::new(&base)
        .query(&[pkg(Ecosystem::Go, "golang.org/x/net", "v0.1.0")])
        .unwrap_err();
    match err {
        Error::Http { url, message } => {
            assert!(url.ends_with("/v1/querybatch"));
            assert!(message.contains("retry later"), "{message}");
        }
        other => panic!("unexpected {other:?}"),
    }
}

// ------------------------------------------------------------ registries

#[test]
fn registries_hit_each_ecosystem_endpoint() {
    let (base, seen) = serve(Box::new(|_, path, _| match path {
        "/npm/@scope%2Futil/latest" => (200, json!({"version": "2.2.0"}).to_string()),
        "/crates/api/v1/crates/serde" => (
            200,
            json!({"crate": {"max_stable_version": "1.0.228", "max_version": "2.0.0-alpha"}})
                .to_string(),
        ),
        "/go/github.com/!burnt!sushi/toml/@latest" => {
            (200, json!({"Version": "v1.4.0"}).to_string())
        }
        "/pub/api/packages/http" => (200, json!({"latest": {"version": "1.2.2"}}).to_string()),
        "/pypi/pypi/django/json" => (200, json!({"info": {"version": "5.1.2"}}).to_string()),
        _ => (404, "{}".into()),
    }));
    let reg = Registries::all_at(&base);
    let cases = [
        (pkg(Ecosystem::Npm, "@scope/util", "2.1.3"), Some("2.2.0")),
        (pkg(Ecosystem::Cargo, "serde", "1.0.190"), Some("1.0.228")),
        (
            pkg(Ecosystem::Go, "github.com/BurntSushi/toml", "v1.3.2"),
            Some("v1.4.0"),
        ),
        (pkg(Ecosystem::Pub, "http", "0.13.5"), Some("1.2.2")),
        (pkg(Ecosystem::PyPI, "django", "3.2.0"), Some("5.1.2")),
        (pkg(Ecosystem::Npm, "does-not-exist", "1.0.0"), None),
    ];
    for (p, want) in cases {
        assert_eq!(reg.latest(&p).unwrap().as_deref(), want, "{}", p.name);
    }
    let seen = seen.lock().unwrap();
    let crates = seen
        .iter()
        .find(|s| s.path.starts_with("/crates/"))
        .unwrap();
    assert!(
        crates.user_agent.contains("tp-deps/") && crates.user_agent.contains("github.com"),
        "crates.io requires an identifying User-Agent: {}",
        crates.user_agent
    );
}

// ------------------------------------------------------------ live

/// Hits the real OSV.dev API. Run with `cargo test -p tp-deps -- --ignored`.
#[test]
#[ignore = "network: queries api.osv.dev"]
fn live_osv_lodash() {
    let advisories = Osv::default()
        .query(&[pkg(Ecosystem::Npm, "lodash", "4.17.20")])
        .unwrap();
    let a = advisories
        .iter()
        .find(|a| a.id == "GHSA-35jh-r3h4-6jhm" || a.aliases.iter().any(|x| x == "CVE-2021-23337"))
        .expect("lodash 4.17.20 command injection advisory");
    assert!(a.severity >= Severity::High, "{a:?}");
    assert!(a.fixed_versions.iter().any(|v| v == "4.17.21"), "{a:?}");
}

/// Hits the five public registries. Run with `cargo test -p tp-deps -- --ignored`.
#[test]
#[ignore = "network: queries npm, crates.io, proxy.golang.org, pub.dev, PyPI"]
fn live_registries() {
    let reg = Registries::default();
    for p in [
        pkg(Ecosystem::Npm, "@types/node", "1.0.0"),
        pkg(Ecosystem::Cargo, "serde", "1.0.0"),
        pkg(Ecosystem::Go, "github.com/BurntSushi/toml", "v1.0.0"),
        pkg(Ecosystem::Pub, "http", "0.13.0"),
        pkg(Ecosystem::PyPI, "django", "3.2.0"),
    ] {
        let latest = reg.latest(&p).unwrap();
        let latest = latest.unwrap_or_else(|| panic!("no latest for {}", p.name));
        assert!(
            tp_deps::version::newer(&latest, &p.version),
            "{} {latest}",
            p.name
        );
    }
}
