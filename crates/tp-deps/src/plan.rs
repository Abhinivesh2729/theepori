//! Upgrade planning: turn packages, advisories and latest versions into a
//! short, ranked list of recommended upgrades.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::{Advisory, Bump, Ecosystem, Package, Reason, Risk, Severity, UpgradeItem, version};

/// Knobs for [`plan`]. The default is the conservative policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlanPolicy {
    /// Propose routine (non-security) upgrades for transitive packages too.
    /// Default `false`: transitive packages only appear for security fixes.
    pub include_transitive: bool,
    /// Propose routine upgrades that cross a major version. Default `true`
    /// (they are flagged breaking / high risk). Security fixes are always
    /// proposed regardless.
    pub allow_major: bool,
    /// Package names never proposed for routine upgrades. Security fixes for
    /// these packages are still reported.
    pub ignore: Vec<String>,
}

impl Default for PlanPolicy {
    fn default() -> Self {
        PlanPolicy {
            include_transitive: false,
            allow_major: true,
            ignore: Vec::new(),
        }
    }
}

/// Draft the recommended upgrade plan.
///
/// Rules:
/// - Security: for each advisory affecting the installed version, take the
///   lowest fixed version newer than the installed one (stable releases
///   preferred); the item targets the highest of those, i.e. the smallest
///   upgrade that resolves every fixable advisory. Advisories with no fixed
///   release produce an item with `to == from` and bump `Unknown` so they stay
///   visible.
/// - Outdated: only for direct dependencies (unless
///   [`PlanPolicy::include_transitive`]), only when the registry's latest is
///   newer, and never to a pre-release from a stable version.
/// - Risk follows the bump: patch → Low, minor → Medium, major or unknown →
///   High; major bumps are `breaking`. Advisory severity raises priority
///   (ordering) but never lowers risk.
/// - Packages that are current produce nothing.
///
/// Items are ordered by priority: security first by severity, then direct
/// before transitive, then by ecosystem and name.
pub fn plan(
    pkgs: &[Package],
    advisories: &[Advisory],
    latest: &HashMap<(Ecosystem, String), String>,
    policy: &PlanPolicy,
) -> Vec<UpgradeItem> {
    // Unique (ecosystem, name, version), direct if any occurrence is direct.
    let mut unique: BTreeMap<(Ecosystem, String, String), bool> = BTreeMap::new();
    for p in pkgs {
        *unique
            .entry((p.ecosystem, p.name.clone(), p.version.clone()))
            .or_default() |= p.direct;
    }

    let mut items = Vec::new();
    for ((eco, name, current), direct) in unique {
        let norm = eco.normalize_name(&name);
        let mut advs: Vec<&Advisory> = advisories
            .iter()
            .filter(|a| {
                a.ecosystem == eco
                    && eco.normalize_name(&a.package) == norm
                    && (a.affected_version.is_empty() || a.affected_version == current)
            })
            .collect();
        advs.sort_by(|a, b| a.id.cmp(&b.id));
        advs.dedup_by(|a, b| a.id == b.id);

        let latest_v = latest
            .get(&(eco, name.clone()))
            .or_else(|| latest.get(&(eco, norm.clone())))
            .map(String::as_str);

        let item = if advs.is_empty() {
            outdated_item(eco, &name, &current, direct, latest_v, policy)
        } else {
            Some(security_item(eco, &name, &current, direct, &advs, latest_v))
        };
        items.extend(item);
    }

    items.sort_by(|a, b| {
        b.is_security()
            .cmp(&a.is_security())
            .then_with(|| b.severity.cmp(&a.severity))
            .then_with(|| b.direct.cmp(&a.direct))
            .then_with(|| a.ecosystem.cmp(&b.ecosystem))
            .then_with(|| a.package.cmp(&b.package))
            .then_with(|| a.from.cmp(&b.from))
    });
    items
}

fn risk_for(bump: Bump) -> Risk {
    match bump {
        Bump::Patch => Risk::Low,
        Bump::Minor => Risk::Medium,
        Bump::Major | Bump::Unknown => Risk::High,
    }
}

fn bump_note(eco: Ecosystem, bump: Bump) -> String {
    let mut s = match bump {
        Bump::Patch => "Patch bump: bug fixes only, low risk.".to_string(),
        Bump::Minor => {
            "Minor bump: new features, should stay backward compatible; medium risk.".to_string()
        }
        Bump::Major => {
            "Major bump: expect breaking changes; read the changelog and run the full test suite."
                .to_string()
        }
        Bump::Unknown => {
            "Versions are not semver-comparable; review the change manually.".to_string()
        }
    };
    if eco == Ecosystem::PyPI && bump != Bump::Unknown {
        s.push_str(" PyPI does not enforce semver, so treat the bump class as a hint.");
    }
    s
}

/// Lowest fixed version newer than `current`, preferring stable releases.
fn lowest_fix<'a>(current: &str, fixed: &'a [String]) -> Option<&'a str> {
    let mut candidates: Vec<&str> = fixed
        .iter()
        .map(String::as_str)
        .filter(|f| version::newer(f, current))
        .collect();
    candidates.sort_by(|a, b| version::compare(a, b).unwrap_or(Ordering::Equal));
    let stable = candidates.iter().find(|f| !version::is_prerelease(f));
    stable.or(candidates.first()).copied()
}

fn security_item(
    eco: Ecosystem,
    name: &str,
    current: &str,
    direct: bool,
    advs: &[&Advisory],
    latest: Option<&str>,
) -> UpgradeItem {
    let severity = advs.iter().map(|a| a.severity).max().unwrap_or_default();
    let mut fixable: Vec<(&Advisory, &str)> = Vec::new();
    let mut unfixed: Vec<&Advisory> = Vec::new();
    for a in advs {
        match lowest_fix(current, &a.fixed_versions) {
            Some(f) => fixable.push((a, f)),
            None => unfixed.push(a),
        }
    }
    let target = fixable
        .iter()
        .map(|(_, f)| *f)
        .max_by(|a, b| version::compare(a, b).unwrap_or(Ordering::Equal));
    let label = |a: &Advisory| format!("{} ({})", a.id, a.severity);
    let ids: Vec<String> = advs.iter().map(|a| a.id.clone()).collect();

    let mut rationale = String::new();
    if severity >= Severity::High {
        rationale.push_str(&format!(
            "Priority: {severity} severity vulnerability, apply promptly. "
        ));
    }
    let (to, bump) = match target {
        Some(to) => {
            let fixed: Vec<String> = fixable.iter().map(|(a, _)| label(a)).collect();
            rationale.push_str(&format!(
                "{name} {current} → {to} is the smallest upgrade that fixes {}. ",
                fixed.join(", ")
            ));
            (to.to_string(), version::bump(current, to))
        }
        None => (current.to_string(), Bump::Unknown),
    };
    if !unfixed.is_empty() {
        let list: Vec<String> = unfixed.iter().map(|a| label(a)).collect();
        rationale.push_str(&format!(
            "No fixed release yet for {}; consider a mitigation or replacing {name}. ",
            list.join(", ")
        ));
    }
    if target.is_some() {
        rationale.push_str(&bump_note(eco, version::bump(current, &to)));
    }

    let mut reasons = vec![Reason::Security(ids)];
    if let Some(l) = latest {
        match version::compare(l, &to) {
            Some(Ordering::Equal) if to != current => reasons.push(Reason::Outdated),
            Some(Ordering::Greater) => rationale.push_str(&format!(
                " Latest is {l} ({} from {to}); upgrade to it separately once the fix lands.",
                version::bump(&to, l)
            )),
            _ => {}
        }
    }
    if !direct {
        rationale.push_str(
            " Transitive dependency: upgrade the direct dependency that pulls it in, or add an override.",
        );
    }

    UpgradeItem {
        package: name.to_string(),
        ecosystem: eco,
        from: current.to_string(),
        to,
        reasons,
        bump,
        risk: risk_for(bump),
        breaking: bump == Bump::Major,
        severity: Some(severity),
        direct,
        rationale: rationale.trim().to_string(),
    }
}

fn outdated_item(
    eco: Ecosystem,
    name: &str,
    current: &str,
    direct: bool,
    latest: Option<&str>,
    policy: &PlanPolicy,
) -> Option<UpgradeItem> {
    if !(direct || policy.include_transitive) {
        return None;
    }
    if policy.ignore.iter().any(|i| i == name) {
        return None;
    }
    let latest = latest?;
    if !version::newer(latest, current) {
        return None; // current (or not comparable): no proposal
    }
    if version::is_prerelease(latest) && !version::is_prerelease(current) {
        return None;
    }
    let bump = version::bump(current, latest);
    if bump == Bump::Major && !policy.allow_major {
        return None;
    }
    let kind = if direct { "Direct" } else { "Transitive" };
    let rationale = format!(
        "{kind} dependency {name} {current} → {latest} (latest release). {}",
        bump_note(eco, bump)
    );
    Some(UpgradeItem {
        package: name.to_string(),
        ecosystem: eco,
        from: current.to_string(),
        to: latest.to_string(),
        reasons: vec![Reason::Outdated],
        bump,
        risk: risk_for(bump),
        breaking: bump == Bump::Major,
        severity: None,
        direct,
        rationale,
    })
}
