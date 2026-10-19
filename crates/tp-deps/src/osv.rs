//! OSV.dev advisory source.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::Duration;

use serde_json::{Value, json};

use crate::severity::{cvss3_base_score, severity_from_label, severity_from_score};
use crate::{Advisory, AdvisorySource, Ecosystem, Package, Result, Severity, http, version};

/// OSV.dev's `querybatch` accepts at most 1000 queries per request.
pub const BATCH_LIMIT: usize = 1000;
/// Upper bound on pagination rounds per batch (defensive).
const MAX_PAGES: usize = 50;

/// OSV.dev client: `/v1/querybatch` to find vulnerability ids per package,
/// then `/v1/vulns/{id}` for details (severity, aliases, fixed versions).
#[derive(Debug, Clone)]
pub struct Osv {
    /// API root, e.g. `https://api.osv.dev` (no trailing `/v1`).
    pub base_url: String,
    pub timeout: Duration,
}

impl Default for Osv {
    fn default() -> Self {
        Osv::new("https://api.osv.dev")
    }
}

impl Osv {
    pub fn new(base_url: impl Into<String>) -> Self {
        Osv {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            timeout: Duration::from_secs(30),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }

    /// Vulnerability ids per package (same order as `pkgs`).
    fn batch_ids(&self, agent: &ureq::Agent, pkgs: &[&Package]) -> Result<Vec<BTreeSet<String>>> {
        let mut ids: Vec<BTreeSet<String>> = vec![BTreeSet::new(); pkgs.len()];
        // (index into pkgs, page token)
        let mut pending: Vec<(usize, Option<String>)> =
            (0..pkgs.len()).map(|i| (i, None)).collect();
        for _ in 0..MAX_PAGES {
            if pending.is_empty() {
                break;
            }
            let queries: Vec<Value> = pending
                .iter()
                .map(|(i, token)| {
                    let p = pkgs[*i];
                    let mut q = json!({
                        "package": {"name": p.name, "ecosystem": p.ecosystem.osv_name()},
                        "version": p.version,
                    });
                    if let Some(t) = token {
                        q["page_token"] = json!(t);
                    }
                    q
                })
                .collect();
            let resp = http::post_json(
                agent,
                &self.url("/v1/querybatch"),
                &json!({ "queries": queries }),
            )?;
            let results = resp
                .get("results")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let mut next = Vec::new();
            for ((i, _), result) in pending.iter().zip(results.iter()) {
                for v in result
                    .get("vulns")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(id) = v.get("id").and_then(Value::as_str) {
                        ids[*i].insert(id.to_string());
                    }
                }
                if let Some(t) = result.get("next_page_token").and_then(Value::as_str)
                    && !t.is_empty()
                {
                    next.push((*i, Some(t.to_string())));
                }
            }
            pending = next;
        }
        Ok(ids)
    }
}

impl AdvisorySource for Osv {
    fn query(&self, pkgs: &[Package]) -> Result<Vec<Advisory>> {
        let agent = http::agent(self.timeout);
        let mut seen = HashSet::new();
        let unique: Vec<&Package> = pkgs
            .iter()
            .filter(|p| seen.insert((p.ecosystem, p.name.as_str(), p.version.as_str())))
            .collect();

        // vuln id -> packages it was reported for
        let mut by_id: BTreeMap<String, Vec<&Package>> = BTreeMap::new();
        for chunk in unique.chunks(BATCH_LIMIT) {
            let ids = self.batch_ids(&agent, chunk)?;
            for (pkg, set) in chunk.iter().zip(ids) {
                for id in set {
                    by_id.entry(id).or_default().push(pkg);
                }
            }
        }

        let mut out = Vec::new();
        for (id, affected) in by_id {
            let url = self.url(&format!("/v1/vulns/{id}"));
            let vuln = http::get_json(&agent, &url)?
                .unwrap_or_else(|| json!({ "id": id, "summary": "details unavailable from OSV" }));
            out.extend(affected.into_iter().map(|p| advisory_from_vuln(&vuln, p)));
        }
        Ok(out)
    }
}

/// Build an [`Advisory`] for `pkg` from an OSV vulnerability document.
///
/// Fixed versions come from `SEMVER`/`ECOSYSTEM` range `fixed` events of the
/// affected entries for this package. Severity is the highest of: CVSS v3
/// base scores (computed from the vector), numeric scores, and
/// `database_specific`/`ecosystem_specific` severity labels.
pub fn advisory_from_vuln(vuln: &Value, pkg: &Package) -> Advisory {
    let str_of = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let id = str_of(vuln, "id");
    let aliases = vuln
        .get("aliases")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    let mut summary = str_of(vuln, "summary");
    if summary.is_empty() {
        summary = str_of(vuln, "details")
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim()
            .chars()
            .take(200)
            .collect();
    }

    let wanted = pkg.ecosystem.normalize_name(&pkg.name);
    let matching: Vec<&Value> = vuln
        .get("affected")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|a| {
            let p = &a["package"];
            let eco = p.get("ecosystem").and_then(Value::as_str).unwrap_or("");
            let eco = eco.split(':').next().unwrap_or("");
            let name = p.get("name").and_then(Value::as_str).unwrap_or("");
            Ecosystem::from_osv_name(eco) == Some(pkg.ecosystem)
                && pkg.ecosystem.normalize_name(name) == wanted
        })
        .collect();

    let mut fixed: Vec<String> = Vec::new();
    for a in &matching {
        for r in a
            .get("ranges")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let kind = r.get("type").and_then(Value::as_str).unwrap_or("");
            if kind != "SEMVER" && kind != "ECOSYSTEM" {
                continue;
            }
            for e in r
                .get("events")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(f) = e.get("fixed").and_then(Value::as_str)
                    && !fixed.iter().any(|x| x == f)
                {
                    fixed.push(f.to_string());
                }
            }
        }
    }
    fixed.sort_by(|a, b| {
        version::compare(a, b)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.cmp(b))
    });

    let mut severity = Severity::Unknown;
    let mut consider = |s: Severity| severity = severity.max(s);
    let mut scored = |list: Option<&Value>| {
        for s in list.and_then(Value::as_array).into_iter().flatten() {
            let score = s.get("score").and_then(Value::as_str).unwrap_or("");
            let value = cvss3_base_score(score).or_else(|| score.trim().parse::<f64>().ok());
            if let Some(v) = value {
                consider(severity_from_score(v));
            }
        }
    };
    scored(vuln.get("severity"));
    for a in &matching {
        scored(a.get("severity"));
    }
    let mut labels = vec![vuln.pointer("/database_specific/severity")];
    for a in &matching {
        labels.push(a.pointer("/database_specific/severity"));
        labels.push(a.pointer("/ecosystem_specific/severity"));
    }
    for l in labels.into_iter().flatten().filter_map(Value::as_str) {
        severity = severity.max(severity_from_label(l));
    }

    Advisory {
        id,
        aliases,
        package: pkg.name.clone(),
        ecosystem: pkg.ecosystem,
        affected_version: pkg.version.clone(),
        severity,
        summary,
        fixed_versions: fixed,
    }
}
