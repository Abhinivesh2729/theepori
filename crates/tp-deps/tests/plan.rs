#![allow(clippy::unwrap_used)]
//! Plan rules, driven through fake advisory and registry sources (no network).

use std::collections::HashMap;

use tp_deps::{
    Advisory, AdvisorySource, Bump, Ecosystem, Package, PlanPolicy, Reason, RegistrySource, Result,
    Risk, Severity, UpgradeItem, collect_latest, plan,
};

struct FakeAdvisories(Vec<Advisory>);

impl AdvisorySource for FakeAdvisories {
    fn query(&self, pkgs: &[Package]) -> Result<Vec<Advisory>> {
        Ok(self
            .0
            .iter()
            .filter(|a| {
                pkgs.iter().any(|p| {
                    p.ecosystem == a.ecosystem
                        && p.name == a.package
                        && p.version == a.affected_version
                })
            })
            .cloned()
            .collect())
    }
}

struct FakeRegistry(HashMap<String, String>);

impl RegistrySource for FakeRegistry {
    fn latest(&self, pkg: &Package) -> Result<Option<String>> {
        if pkg.name == "explodes" {
            return Err(tp_deps::Error::Http {
                url: "fake://explodes".into(),
                message: "boom".into(),
            });
        }
        Ok(self.0.get(&pkg.name).cloned())
    }
}

fn pkg(eco: Ecosystem, name: &str, version: &str, direct: bool) -> Package {
    Package {
        name: name.into(),
        version: version.into(),
        ecosystem: eco,
        direct,
    }
}

fn adv(id: &str, p: &Package, sev: Severity, fixed: &[&str]) -> Advisory {
    Advisory {
        id: id.into(),
        aliases: vec![],
        package: p.name.clone(),
        ecosystem: p.ecosystem,
        affected_version: p.version.clone(),
        severity: sev,
        summary: format!("{id} summary"),
        fixed_versions: fixed.iter().map(|s| s.to_string()).collect(),
    }
}

/// Run the full pipeline against the fakes with the given policy.
fn run(
    pkgs: &[Package],
    advisories: Vec<Advisory>,
    latest: &[(&str, &str)],
    policy: &PlanPolicy,
) -> Vec<UpgradeItem> {
    let advs = FakeAdvisories(advisories).query(pkgs).unwrap();
    let reg = FakeRegistry(
        latest
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    );
    let (latest, errors) = collect_latest(&reg, pkgs, !policy.include_transitive);
    assert!(errors.is_empty());
    plan(pkgs, &advs, &latest, policy)
}

fn find<'a>(items: &'a [UpgradeItem], name: &str) -> &'a UpgradeItem {
    items
        .iter()
        .find(|i| i.package == name)
        .unwrap_or_else(|| panic!("no item for {name}"))
}

#[test]
fn security_picks_lowest_fixed_version_above_current() {
    let p = pkg(Ecosystem::Npm, "lodash", "4.17.15", true);
    // Fixes on several lines; lowest above current is 4.17.19.
    let a = adv(
        "GHSA-1",
        &p,
        Severity::Medium,
        &["3.10.2", "4.17.19", "4.17.21", "5.0.1"],
    );
    let items = run(
        &[p],
        vec![a],
        &[("lodash", "5.0.1")],
        &PlanPolicy::default(),
    );
    assert_eq!(items.len(), 1);
    let i = &items[0];
    assert_eq!((i.from.as_str(), i.to.as_str()), ("4.17.15", "4.17.19"));
    assert_eq!(i.bump, Bump::Patch);
    assert_eq!(i.risk, Risk::Low);
    assert!(!i.breaking);
    assert_eq!(i.reasons, vec![Reason::Security(vec!["GHSA-1".into()])]);
    assert!(i.rationale.contains("Latest is 5.0.1"), "{}", i.rationale);
}

#[test]
fn multiple_advisories_take_the_smallest_version_fixing_all() {
    let p = pkg(Ecosystem::Cargo, "smallvec", "1.6.0", false);
    let a1 = adv("RUSTSEC-A", &p, Severity::Low, &["1.6.1"]);
    let a2 = adv("RUSTSEC-B", &p, Severity::High, &["0.6.14", "1.8.2"]);
    let items = run(&[p], vec![a1, a2], &[], &PlanPolicy::default());
    let i = &items[0];
    assert_eq!(i.to, "1.8.2");
    assert_eq!(i.bump, Bump::Minor);
    assert_eq!(i.risk, Risk::Medium);
    assert_eq!(i.severity, Some(Severity::High));
    assert_eq!(
        i.advisory_ids(),
        vec!["RUSTSEC-A", "RUSTSEC-B"],
        "both advisories are addressed"
    );
    assert!(i.rationale.starts_with("Priority: high"), "{}", i.rationale);
    assert!(i.rationale.contains("Transitive dependency"));
}

#[test]
fn major_bumps_are_breaking_and_high_risk() {
    let p = pkg(Ecosystem::Npm, "express", "3.21.2", true);
    let items = run(
        &[p],
        vec![],
        &[("express", "4.19.2")],
        &PlanPolicy::default(),
    );
    let i = &items[0];
    assert_eq!(i.bump, Bump::Major);
    assert_eq!(i.risk, Risk::High);
    assert!(i.breaking);
    assert_eq!(i.reasons, vec![Reason::Outdated]);
}

#[test]
fn zero_major_minor_bump_is_breaking() {
    let p = pkg(Ecosystem::Cargo, "toml", "0.7.8", true);
    let items = run(&[p], vec![], &[("toml", "0.8.2")], &PlanPolicy::default());
    assert_eq!(items[0].bump, Bump::Major);
    assert!(items[0].breaking);
}

#[test]
fn risk_follows_bump_for_minor_and_patch() {
    let pkgs = [
        pkg(Ecosystem::Pub, "http", "0.13.5", true),
        pkg(Ecosystem::PyPI, "requests", "2.28.1", true),
        pkg(Ecosystem::Go, "github.com/gin-gonic/gin", "v1.9.0", true),
    ];
    let items = run(
        &pkgs,
        vec![],
        &[
            ("http", "0.13.6"),
            ("requests", "2.32.3"),
            ("github.com/gin-gonic/gin", "v1.9.1"),
        ],
        &PlanPolicy::default(),
    );
    assert_eq!(items.len(), 3);
    assert_eq!(
        (find(&items, "http").bump, find(&items, "http").risk),
        (Bump::Patch, Risk::Low)
    );
    let req = find(&items, "requests");
    assert_eq!((req.bump, req.risk), (Bump::Minor, Risk::Medium));
    assert!(req.rationale.contains("PyPI does not enforce semver"));
    let gin = find(&items, "github.com/gin-gonic/gin");
    assert_eq!(
        (gin.bump, gin.risk, gin.to.as_str()),
        (Bump::Patch, Risk::Low, "v1.9.1")
    );
}

#[test]
fn critical_vulnerability_raises_priority_but_not_lowers_risk() {
    let crit = pkg(Ecosystem::Npm, "aaa-crit", "1.0.0", true);
    let outdated = pkg(Ecosystem::Npm, "zzz-patch", "1.0.0", true);
    let a = adv("GHSA-CRIT", &crit, Severity::Critical, &["2.0.0"]);
    let items = run(
        &[outdated, crit],
        vec![a],
        &[("zzz-patch", "1.0.1"), ("aaa-crit", "2.0.0")],
        &PlanPolicy::default(),
    );
    assert_eq!(items[0].package, "aaa-crit", "security items come first");
    let i = &items[0];
    assert_eq!(i.severity, Some(Severity::Critical));
    assert_eq!(i.risk, Risk::High, "major fix stays high risk");
    assert!(i.breaking);
    assert!(i.rationale.starts_with("Priority: critical"));
    // Target equals latest, so the item also makes the package current.
    assert_eq!(
        i.reasons,
        vec![Reason::Security(vec!["GHSA-CRIT".into()]), Reason::Outdated]
    );

    // A critical advisory fixed by a patch release is still Low risk: severity
    // never inflates or deflates risk, it orders the plan.
    let p = pkg(Ecosystem::Npm, "patchy", "1.0.0", true);
    let a = adv("GHSA-P", &p, Severity::Critical, &["1.0.1"]);
    let items = run(&[p], vec![a], &[], &PlanPolicy::default());
    assert_eq!(items[0].risk, Risk::Low);
}

#[test]
fn priority_ordering() {
    let low = pkg(Ecosystem::Npm, "a-low", "1.0.0", true);
    let high = pkg(Ecosystem::Npm, "b-high", "1.0.0", false);
    let crit = pkg(Ecosystem::Npm, "c-crit", "1.0.0", false);
    let plain = pkg(Ecosystem::Npm, "0-plain", "1.0.0", true);
    let advs = vec![
        adv("L", &low, Severity::Low, &["1.0.1"]),
        adv("H", &high, Severity::High, &["1.0.1"]),
        adv("C", &crit, Severity::Critical, &["1.0.1"]),
    ];
    let items = run(
        &[low, high, crit, plain],
        advs,
        &[("0-plain", "1.1.0")],
        &PlanPolicy::default(),
    );
    let order: Vec<&str> = items.iter().map(|i| i.package.as_str()).collect();
    assert_eq!(order, vec!["c-crit", "b-high", "a-low", "0-plain"]);
}

#[test]
fn only_direct_deps_get_outdated_items_by_default() {
    let pkgs = [
        pkg(Ecosystem::Npm, "direct", "1.0.0", true),
        pkg(Ecosystem::Npm, "transitive", "1.0.0", false),
    ];
    let latest = [("direct", "1.1.0"), ("transitive", "1.1.0")];
    let items = run(&pkgs, vec![], &latest, &PlanPolicy::default());
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].package, "direct");

    let wide = PlanPolicy {
        include_transitive: true,
        ..PlanPolicy::default()
    };
    let items = run(&pkgs, vec![], &latest, &wide);
    assert_eq!(items.len(), 2);
    assert!(
        find(&items, "transitive")
            .rationale
            .starts_with("Transitive")
    );
}

#[test]
fn current_packages_produce_nothing() {
    let pkgs = [
        pkg(Ecosystem::Cargo, "serde", "1.0.228", true),
        pkg(Ecosystem::Go, "golang.org/x/text", "v0.3.7", true),
        pkg(Ecosystem::Pub, "provider", "6.0.5+1", true),
        pkg(Ecosystem::PyPI, "ahead", "2.0.0", true),
        pkg(Ecosystem::Npm, "unknown-to-registry", "1.0.0", true),
    ];
    let items = run(
        &pkgs,
        vec![],
        &[
            ("serde", "1.0.228"),
            ("golang.org/x/text", "v0.3.7"),
            ("provider", "6.0.5"), // build metadata only
            ("ahead", "1.9.0"),    // installed is newer than registry latest
        ],
        &PlanPolicy::default(),
    );
    assert!(items.is_empty(), "{items:#?}");
}

#[test]
fn prerelease_latest_is_not_proposed_for_stable_installs() {
    let p = pkg(Ecosystem::Npm, "next", "14.2.0", true);
    let items = run(
        &[p],
        vec![],
        &[("next", "15.0.0-rc.1")],
        &PlanPolicy::default(),
    );
    assert!(items.is_empty());
}

#[test]
fn policy_can_forbid_major_outdated_and_ignore_packages_but_not_security() {
    let pkgs = [
        pkg(Ecosystem::Npm, "react", "17.0.2", true),
        pkg(Ecosystem::Npm, "pinned", "1.0.0", true),
    ];
    let a = adv("GHSA-PIN", &pkgs[1], Severity::Medium, &["1.0.3"]);
    let policy = PlanPolicy {
        allow_major: false,
        ignore: vec!["pinned".into()],
        ..PlanPolicy::default()
    };
    let items = run(
        &pkgs,
        vec![a],
        &[("react", "18.3.1"), ("pinned", "1.2.0")],
        &policy,
    );
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].package, "pinned");
    assert!(items[0].is_security());
    assert_eq!(items[0].to, "1.0.3");
}

#[test]
fn unfixed_advisory_stays_visible_without_proposing_a_version() {
    let p = pkg(Ecosystem::PyPI, "abandoned", "0.4.0", true);
    let a = adv("PYSEC-X", &p, Severity::High, &[]);
    let items = run(
        &[p],
        vec![a],
        &[("abandoned", "0.4.0")],
        &PlanPolicy::default(),
    );
    let i = &items[0];
    assert_eq!(i.to, i.from);
    assert_eq!(i.bump, Bump::Unknown);
    assert_eq!(i.risk, Risk::High);
    assert!(!i.breaking);
    assert!(i.rationale.contains("No fixed release yet for PYSEC-X"));
    assert_eq!(i.reasons, vec![Reason::Security(vec!["PYSEC-X".into()])]);
}

#[test]
fn stable_fix_preferred_over_prerelease() {
    let p = pkg(Ecosystem::Cargo, "h2", "0.3.20", true);
    let a = adv(
        "RUSTSEC-H2",
        &p,
        Severity::High,
        &["0.3.21-alpha.1", "0.3.24"],
    );
    let items = run(&[p], vec![a], &[], &PlanPolicy::default());
    assert_eq!(items[0].to, "0.3.24");
}

#[test]
fn go_and_pub_versions_compare_leniently() {
    let go = pkg(
        Ecosystem::Go,
        "golang.org/x/net",
        "v0.0.0-20220722155237-a158d28d115b",
        false,
    );
    let pubp = pkg(Ecosystem::Pub, "provider", "6.0.5+1", true);
    let advs = vec![
        adv("GO-2022-1", &go, Severity::Medium, &["0.7.0"]),
        adv("PUB-1", &pubp, Severity::Low, &["6.0.6"]),
    ];
    let items = run(&[go, pubp], advs, &[], &PlanPolicy::default());
    let net = find(&items, "golang.org/x/net");
    assert_eq!(net.to, "0.7.0");
    assert_eq!(
        net.bump,
        Bump::Major,
        "0.0.x → 0.7.0 is breaking under caret rules"
    );
    let prov = find(&items, "provider");
    assert_eq!((prov.to.as_str(), prov.bump), ("6.0.6", Bump::Patch));
}

#[test]
fn pypi_advisory_names_match_after_normalization() {
    let p = pkg(Ecosystem::PyPI, "pyyaml", "5.3.1", true);
    let mut a = adv("PYSEC-Y", &p, Severity::Critical, &["5.4"]);
    a.package = "PyYAML".into();
    let items = plan(
        std::slice::from_ref(&p),
        &[a],
        &HashMap::new(),
        &PlanPolicy::default(),
    );
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].to, "5.4");
    assert_eq!(items[0].bump, Bump::Minor);
}

#[test]
fn duplicate_packages_are_planned_once() {
    let pkgs = [
        pkg(Ecosystem::Npm, "lodash", "4.17.20", false),
        pkg(Ecosystem::Npm, "lodash", "4.17.20", true),
    ];
    let items = run(
        &pkgs,
        vec![],
        &[("lodash", "4.17.21")],
        &PlanPolicy::default(),
    );
    assert_eq!(items.len(), 1);
    assert!(items[0].direct);
}

#[test]
fn registry_errors_are_collected_not_fatal() {
    let reg = FakeRegistry(HashMap::from([("ok".to_string(), "2.0.0".to_string())]));
    let pkgs = [
        pkg(Ecosystem::Npm, "ok", "1.0.0", true),
        pkg(Ecosystem::Npm, "explodes", "1.0.0", true),
        pkg(Ecosystem::Npm, "transitive", "1.0.0", false),
    ];
    let (latest, errors) = collect_latest(&reg, &pkgs, true);
    assert_eq!(latest.len(), 1);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].0.name, "explodes");
}

#[test]
fn plan_serializes() {
    let p = pkg(Ecosystem::Npm, "lodash", "4.17.20", true);
    let a = adv("GHSA-1", &p, Severity::High, &["4.17.21"]);
    let items = run(
        &[p],
        vec![a],
        &[("lodash", "4.17.21")],
        &PlanPolicy::default(),
    );
    let json = serde_json::to_value(&items).unwrap();
    assert_eq!(json[0]["bump"], "patch");
    assert_eq!(json[0]["risk"], "low");
    assert_eq!(json[0]["severity"], "high");
    assert_eq!(json[0]["reasons"][0]["kind"], "security");
    assert_eq!(json[0]["reasons"][0]["advisories"][0], "GHSA-1");
    assert_eq!(json[0]["reasons"][1]["kind"], "outdated");
    let back: Vec<UpgradeItem> = serde_json::from_value(json).unwrap();
    assert_eq!(back, items);
    let policy: PlanPolicy = serde_json::from_str("{\"include_transitive\": true}").unwrap();
    assert!(policy.include_transitive && policy.allow_major);
}
