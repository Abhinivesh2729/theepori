//! Registry lookups for the latest stable version of a package.

use std::time::Duration;

use serde_json::Value;

use crate::{Ecosystem, Error, LatestVersions, Package, RegistrySource, Result, http};

/// Public registries. Each base URL is overridable (mirrors, tests).
#[derive(Debug, Clone)]
pub struct Registries {
    /// `https://registry.npmjs.org` → `GET {npm}/{name}/latest`
    pub npm: String,
    /// `https://crates.io` → `GET {crates}/api/v1/crates/{name}` (sends a User-Agent, as crates.io requires)
    pub crates: String,
    /// `https://proxy.golang.org` → `GET {go}/{escaped module}/@latest`
    pub go: String,
    /// `https://pub.dev` → `GET {pub_dev}/api/packages/{name}`
    pub pub_dev: String,
    /// `https://pypi.org` → `GET {pypi}/pypi/{name}/json`
    pub pypi: String,
    pub timeout: Duration,
}

impl Default for Registries {
    fn default() -> Self {
        Registries {
            npm: "https://registry.npmjs.org".into(),
            crates: "https://crates.io".into(),
            go: "https://proxy.golang.org".into(),
            pub_dev: "https://pub.dev".into(),
            pypi: "https://pypi.org".into(),
            timeout: Duration::from_secs(20),
        }
    }
}

impl Registries {
    /// Point every registry at one base URL (for a local test server).
    pub fn all_at(base: &str) -> Self {
        let base = base.trim_end_matches('/').to_string();
        Registries {
            npm: format!("{base}/npm"),
            crates: format!("{base}/crates"),
            go: format!("{base}/go"),
            pub_dev: format!("{base}/pub"),
            pypi: format!("{base}/pypi"),
            timeout: Duration::from_secs(5),
        }
    }

    /// The URL queried for `pkg`.
    pub fn url_for(&self, pkg: &Package) -> String {
        let t = |s: &str| s.trim_end_matches('/').to_string();
        match pkg.ecosystem {
            Ecosystem::Npm => format!("{}/{}/latest", t(&self.npm), pkg.name.replace('/', "%2F")),
            Ecosystem::Cargo => format!("{}/api/v1/crates/{}", t(&self.crates), pkg.name),
            Ecosystem::Go => format!("{}/{}/@latest", t(&self.go), go_escape(&pkg.name)),
            Ecosystem::Pub => format!("{}/api/packages/{}", t(&self.pub_dev), pkg.name),
            Ecosystem::PyPI => format!("{}/pypi/{}/json", t(&self.pypi), pkg.name),
        }
    }
}

/// Extract the latest stable version from a registry response.
pub(crate) fn latest_from_response(eco: Ecosystem, doc: &Value) -> Option<String> {
    let s = |p: &str| {
        doc.pointer(p)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
    };
    let v = match eco {
        Ecosystem::Npm => s("/version"),
        Ecosystem::Cargo => s("/crate/max_stable_version").or_else(|| s("/crate/max_version")),
        Ecosystem::Go => s("/Version"),
        Ecosystem::Pub => s("/latest/version"),
        Ecosystem::PyPI => s("/info/version"),
    };
    v.map(str::to_string)
}

impl RegistrySource for Registries {
    fn latest(&self, pkg: &Package) -> Result<Option<String>> {
        let url = self.url_for(pkg);
        let agent = http::agent(self.timeout);
        Ok(http::get_json(&agent, &url)?.and_then(|doc| latest_from_response(pkg.ecosystem, &doc)))
    }
}

/// proxy.golang.org module path escaping: each uppercase letter becomes
/// `!` followed by its lowercase form.
fn go_escape(module: &str) -> String {
    let mut out = String::with_capacity(module.len());
    for c in module.chars() {
        if c.is_ascii_uppercase() {
            out.push('!');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Look up the latest version of each unique (ecosystem, name) in `pkgs`.
/// When `direct_only`, transitive packages are skipped (fewer requests).
/// Failures are returned alongside the results instead of aborting the run.
pub fn collect_latest(
    src: &dyn RegistrySource,
    pkgs: &[Package],
    direct_only: bool,
) -> (LatestVersions, Vec<(Package, Error)>) {
    let mut latest = LatestVersions::new();
    let mut errors = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for p in pkgs {
        if direct_only && !p.direct {
            continue;
        }
        if !seen.insert((p.ecosystem, p.name.clone())) {
            continue;
        }
        match src.latest(p) {
            Ok(Some(v)) => {
                latest.insert((p.ecosystem, p.name.clone()), v);
            }
            Ok(None) => {}
            Err(e) => errors.push((p.clone(), e)),
        }
    }
    (latest, errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pkg(eco: Ecosystem, name: &str) -> Package {
        Package {
            name: name.into(),
            version: "1.0.0".into(),
            ecosystem: eco,
            direct: true,
        }
    }

    #[test]
    fn urls() {
        let r = Registries::default();
        assert_eq!(
            r.url_for(&pkg(Ecosystem::Npm, "@types/node")),
            "https://registry.npmjs.org/@types%2Fnode/latest"
        );
        assert_eq!(
            r.url_for(&pkg(Ecosystem::Go, "github.com/BurntSushi/toml")),
            "https://proxy.golang.org/github.com/!burnt!sushi/toml/@latest"
        );
        assert_eq!(
            r.url_for(&pkg(Ecosystem::Cargo, "serde")),
            "https://crates.io/api/v1/crates/serde"
        );
        assert_eq!(
            r.url_for(&pkg(Ecosystem::Pub, "http")),
            "https://pub.dev/api/packages/http"
        );
        assert_eq!(
            r.url_for(&pkg(Ecosystem::PyPI, "django")),
            "https://pypi.org/pypi/django/json"
        );
    }

    #[test]
    fn response_shapes() {
        let cases = [
            (
                Ecosystem::Npm,
                json!({"name": "x", "version": "4.17.21"}),
                "4.17.21",
            ),
            (
                Ecosystem::Cargo,
                json!({"crate": {"max_version": "2.0.0-rc.1", "max_stable_version": "1.0.228"}}),
                "1.0.228",
            ),
            (
                Ecosystem::Go,
                json!({"Version": "v1.5.0", "Time": "x"}),
                "v1.5.0",
            ),
            (
                Ecosystem::Pub,
                json!({"latest": {"version": "1.2.0"}}),
                "1.2.0",
            ),
            (
                Ecosystem::PyPI,
                json!({"info": {"version": "5.0.1"}}),
                "5.0.1",
            ),
        ];
        for (eco, doc, want) in cases {
            assert_eq!(
                latest_from_response(eco, &doc).as_deref(),
                Some(want),
                "{eco}"
            );
        }
        assert_eq!(latest_from_response(Ecosystem::Npm, &json!({})), None);
    }
}
