//! Lockfile and manifest parsers.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use crate::{Ecosystem, Error, Manifest, Package, Result};

/// Resolved packages for one manifest.
///
/// Versions come from the lockfile (or, for Go and `requirements.txt`, from
/// the manifest itself, which pins them). `direct` marks packages the
/// manifest names. Without a lockfile, npm, Cargo and pub manifests yield
/// nothing (resolved versions are unknown until the lockfile is generated)
/// and Python manifests yield only their exact `==` pins.
///
/// Local, path, git and SDK packages are skipped: they are not published to
/// the ecosystem's registry, so neither advisories nor upgrades apply.
pub fn packages(m: &Manifest) -> Result<Vec<Package>> {
    let pkgs = match (m.ecosystem, &m.lockfile) {
        (Ecosystem::Npm, Some(lock)) => {
            let direct = npm_direct_names(&m.path)?;
            npm_lock(lock, &direct)?
        }
        (Ecosystem::Cargo, Some(lock)) => cargo_lock(lock, &m.path)?,
        (Ecosystem::Go, _) => go_mod(&m.path)?,
        (Ecosystem::Pub, Some(lock)) => {
            let direct = pubspec_direct_names(&m.path)?;
            pubspec_lock(lock, &direct)?
        }
        (Ecosystem::PyPI, lock) => {
            let is_pyproject = m
                .path
                .file_name()
                .is_some_and(|n| n.to_string_lossy() == "pyproject.toml");
            let reqs = if is_pyproject {
                pyproject_requirements(&m.path)?
            } else {
                requirements_txt(&m.path)?
            };
            match lock {
                Some(lock) => {
                    let direct: HashSet<String> = reqs.into_iter().map(|(n, _)| n).collect();
                    poetry_lock(lock, &direct)?
                }
                None => reqs
                    .into_iter()
                    .filter_map(|(name, pin)| {
                        pin.map(|version| Package {
                            name,
                            version,
                            ecosystem: Ecosystem::PyPI,
                            direct: true,
                        })
                    })
                    .collect(),
            }
        }
        (Ecosystem::Npm | Ecosystem::Cargo | Ecosystem::Pub, None) => Vec::new(),
    };
    Ok(dedupe(pkgs))
}

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Merge duplicate (name, version) entries, keeping `direct` if any copy is direct.
fn dedupe(pkgs: Vec<Package>) -> Vec<Package> {
    let mut map: BTreeMap<(String, String), Package> = BTreeMap::new();
    for p in pkgs {
        map.entry((p.name.clone(), p.version.clone()))
            .and_modify(|e| e.direct |= p.direct)
            .or_insert(p);
    }
    map.into_values().collect()
}

// ---------------------------------------------------------------- npm

fn npm_direct_names(manifest: &Path) -> Result<HashSet<String>> {
    let text = read(manifest)?;
    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| Error::parse(manifest, format!("invalid JSON: {e}")))?;
    let mut out = HashSet::new();
    for key in [
        "dependencies",
        "devDependencies",
        "optionalDependencies",
        "peerDependencies",
    ] {
        if let Some(obj) = json.get(key).and_then(|v| v.as_object()) {
            out.extend(obj.keys().cloned());
        }
    }
    Ok(out)
}

/// package-lock.json v2/v3 (`packages` map keyed by install path).
fn npm_lock(lock: &Path, direct: &HashSet<String>) -> Result<Vec<Package>> {
    let text = read(lock)?;
    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| Error::parse(lock, format!("invalid JSON: {e}")))?;
    let Some(packages) = json.get("packages").and_then(|v| v.as_object()) else {
        let v = json
            .get("lockfileVersion")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        return Err(Error::parse(
            lock,
            format!(
                "lockfileVersion {v} has no \"packages\" map; regenerate it with npm 7 or newer (`npm install --package-lock-only`)"
            ),
        ));
    };
    let mut out = Vec::new();
    for (key, entry) in packages {
        // "" is the root project; keys without node_modules/ are workspace sources.
        let Some(idx) = key.rfind("node_modules/") else {
            continue;
        };
        if entry.get("link").and_then(|v| v.as_bool()) == Some(true) {
            continue;
        }
        let Some(version) = entry.get("version").and_then(|v| v.as_str()) else {
            continue;
        };
        let install_name = &key[idx + "node_modules/".len()..];
        // Aliases ("foo": "npm:bar@1") record the real name in "name".
        let name = entry
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(install_name);
        let top_level = idx == 0;
        out.push(Package {
            name: name.to_string(),
            version: version.to_string(),
            ecosystem: Ecosystem::Npm,
            direct: top_level && direct.contains(install_name),
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------- Cargo

fn parse_toml(path: &Path) -> Result<toml::Table> {
    let text = read(path)?;
    text.parse::<toml::Table>()
        .map_err(|e| Error::parse(path, format!("invalid TOML: {e}")))
}

/// Dependency names declared in a Cargo.toml (all dependency tables,
/// target-specific tables and `[workspace.dependencies]`), honoring
/// `package = "..."` renames.
fn cargo_manifest_names(manifest: &toml::Table) -> HashSet<String> {
    fn collect(table: &toml::Table, out: &mut HashSet<String>) {
        for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
            if let Some(deps) = table.get(key).and_then(|v| v.as_table()) {
                for (name, spec) in deps {
                    let real = spec.get("package").and_then(|v| v.as_str()).unwrap_or(name);
                    out.insert(real.to_string());
                }
            }
        }
    }
    let mut out = HashSet::new();
    collect(manifest, &mut out);
    if let Some(targets) = manifest.get("target").and_then(|v| v.as_table()) {
        for t in targets.values().filter_map(|v| v.as_table()) {
            collect(t, &mut out);
        }
    }
    if let Some(ws) = manifest.get("workspace").and_then(|v| v.as_table()) {
        collect(ws, &mut out);
    }
    out
}

fn cargo_lock(lock: &Path, manifest_path: &Path) -> Result<Vec<Package>> {
    let lock_toml = parse_toml(lock)?;
    let manifest = parse_toml(manifest_path)?;
    let Some(entries) = lock_toml.get("package").and_then(|v| v.as_array()) else {
        return Ok(Vec::new());
    };

    // Local crates (no `source`) whose dependency lists define "direct".
    let is_workspace = manifest.contains_key("workspace");
    let own_name = manifest
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|v| v.as_str());
    let mut direct: Vec<(String, Option<String>)> = Vec::new();
    let mut found_local = false;
    for e in entries {
        if e.get("source").is_some() {
            continue;
        }
        let name = e.get("name").and_then(|v| v.as_str()).unwrap_or_default();
        if !(is_workspace || Some(name) == own_name) {
            continue;
        }
        found_local = true;
        for d in e
            .get("dependencies")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
        {
            // "name", "name version" or "name version (source)".
            let mut parts = d.split_whitespace();
            if let Some(n) = parts.next() {
                direct.push((n.to_string(), parts.next().map(str::to_string)));
            }
        }
    }
    if !found_local {
        direct = cargo_manifest_names(&manifest)
            .into_iter()
            .map(|n| (n, None))
            .collect();
    }

    let mut out = Vec::new();
    for e in entries {
        let Some(source) = e.get("source").and_then(|v| v.as_str()) else {
            continue;
        };
        if !(source.starts_with("registry+") || source.starts_with("sparse+")) {
            continue; // git dependencies are not on crates.io
        }
        let (Some(name), Some(version)) = (
            e.get("name").and_then(|v| v.as_str()),
            e.get("version").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        let is_direct = direct
            .iter()
            .any(|(n, v)| n == name && v.as_deref().is_none_or(|v| v == version));
        out.push(Package {
            name: name.to_string(),
            version: version.to_string(),
            ecosystem: Ecosystem::Cargo,
            direct: is_direct,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------- Go

/// go.mod `require` directives (single-line and blocks). `// indirect` marks
/// transitive requirements.
fn go_mod(path: &Path) -> Result<Vec<Package>> {
    let text = read(path)?;
    let mut out = Vec::new();
    let mut block: Option<String> = None;
    for raw in text.lines() {
        let (code, comment) = match raw.find("//") {
            Some(i) => (&raw[..i], &raw[i + 2..]),
            None => (raw, ""),
        };
        let code = code.trim();
        let indirect = comment
            .split_whitespace()
            .any(|w| w == "indirect;" || w == "indirect");
        if code.is_empty() {
            continue;
        }
        if let Some(kind) = &block {
            if code == ")" {
                block = None;
                continue;
            }
            if kind == "require" {
                push_go_require(code, indirect, &mut out);
            }
            continue;
        }
        let mut words = code.splitn(2, char::is_whitespace);
        let directive = words.next().unwrap_or_default();
        let rest = words.next().unwrap_or_default().trim();
        if rest == "(" {
            block = Some(directive.to_string());
        } else if directive == "require" {
            push_go_require(rest, indirect, &mut out);
        }
    }
    Ok(out)
}

fn push_go_require(spec: &str, indirect: bool, out: &mut Vec<Package>) {
    let mut parts = spec.split_whitespace();
    if let (Some(module), Some(version)) = (parts.next(), parts.next()) {
        out.push(Package {
            name: module.trim_matches('"').to_string(),
            version: version.to_string(),
            ecosystem: Ecosystem::Go,
            direct: !indirect,
        });
    }
}

// ---------------------------------------------------------------- pub

fn parse_yaml(path: &Path) -> Result<serde_yaml::Value> {
    let text = read(path)?;
    serde_yaml::from_str(&text).map_err(|e| Error::parse(path, format!("invalid YAML: {e}")))
}

fn pubspec_direct_names(manifest: &Path) -> Result<HashSet<String>> {
    let yaml = parse_yaml(manifest)?;
    let mut out = HashSet::new();
    for key in ["dependencies", "dev_dependencies", "dependency_overrides"] {
        if let Some(map) = yaml.get(key).and_then(|v| v.as_mapping()) {
            out.extend(map.keys().filter_map(|k| k.as_str()).map(str::to_string));
        }
    }
    Ok(out)
}

fn pubspec_lock(lock: &Path, direct: &HashSet<String>) -> Result<Vec<Package>> {
    let yaml = parse_yaml(lock)?;
    let Some(packages) = yaml.get("packages").and_then(|v| v.as_mapping()) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for (name, entry) in packages {
        let Some(name) = name.as_str() else { continue };
        if entry.get("source").and_then(|v| v.as_str()) != Some("hosted") {
            continue; // sdk, path and git packages are not on pub.dev
        }
        let Some(version) = entry.get("version").and_then(|v| v.as_str()) else {
            continue;
        };
        let lock_direct = entry
            .get("dependency")
            .and_then(|v| v.as_str())
            .is_some_and(|d| d.starts_with("direct"));
        out.push(Package {
            name: name.to_string(),
            version: version.to_string(),
            ecosystem: Ecosystem::Pub,
            direct: lock_direct || direct.contains(name),
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------- Python

/// Parse one PEP 508 requirement into (normalized name, exact `==` pin).
/// Returns `None` for options, URLs-only lines and blanks.
pub(crate) fn pep508(spec: &str) -> Option<(String, Option<String>)> {
    let spec = spec.split(';').next()?.trim(); // drop environment markers
    if spec.is_empty() || spec.starts_with('-') {
        return None;
    }
    let name_end = spec
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
        .unwrap_or(spec.len());
    let name = &spec[..name_end];
    if name.is_empty() {
        return None;
    }
    let mut rest = spec[name_end..].trim_start();
    if rest.starts_with('[') {
        rest = rest.find(']').map_or("", |i| rest[i + 1..].trim_start());
    }
    if rest.starts_with('@') {
        // direct URL reference: not a registry pin
        return Some((Ecosystem::PyPI.normalize_name(name), None));
    }
    if !(rest.is_empty() || rest.starts_with(['=', '<', '>', '!', '~', '('])) {
        return None; // bare URL or path, not a named requirement
    }
    let rest = rest.trim_start_matches('(').trim_end_matches(')');
    let pin = rest
        .split(',')
        .filter_map(|clause| {
            let clause = clause.trim();
            let v = clause
                .strip_prefix("===")
                .or_else(|| clause.strip_prefix("=="))?
                .trim();
            (!v.is_empty() && !v.contains('*')).then(|| v.to_string())
        })
        .next();
    Some((Ecosystem::PyPI.normalize_name(name), pin))
}

/// `requirements.txt`: every requirement line, with its `==` pin if any.
fn requirements_txt(path: &Path) -> Result<Vec<(String, Option<String>)>> {
    let text = read(path)?;
    let mut out = Vec::new();
    let mut logical = String::new();
    for raw in text.lines() {
        let line = match raw.find(" #") {
            Some(i) => &raw[..i],
            None if raw.trim_start().starts_with('#') => "",
            None => raw,
        };
        if let Some(cont) = line.trim_end().strip_suffix('\\') {
            logical.push_str(cont);
            logical.push(' ');
            continue;
        }
        logical.push_str(line);
        let full = std::mem::take(&mut logical);
        // strip trailing per-requirement options such as --hash=...
        let req = full.split(" --").next().unwrap_or_default().trim();
        if let Some(r) = pep508(req) {
            out.push(r);
        }
    }
    Ok(out)
}

/// Direct requirements from pyproject.toml: Poetry tables, PEP 621
/// `[project]` dependencies and PEP 735 `[dependency-groups]`.
fn pyproject_requirements(path: &Path) -> Result<Vec<(String, Option<String>)>> {
    let doc = parse_toml(path)?;
    let mut out = Vec::new();
    let poetry = doc
        .get("tool")
        .and_then(|t| t.get("poetry"))
        .and_then(|v| v.as_table());
    if let Some(poetry) = poetry {
        let mut tables: Vec<&toml::Table> = Vec::new();
        for key in ["dependencies", "dev-dependencies"] {
            if let Some(t) = poetry.get(key).and_then(|v| v.as_table()) {
                tables.push(t);
            }
        }
        if let Some(groups) = poetry.get("group").and_then(|v| v.as_table()) {
            for g in groups.values() {
                if let Some(t) = g.get("dependencies").and_then(|v| v.as_table()) {
                    tables.push(t);
                }
            }
        }
        for t in tables {
            for (name, spec) in t {
                if name.eq_ignore_ascii_case("python") {
                    continue;
                }
                let version = spec
                    .as_str()
                    .or_else(|| spec.get("version").and_then(|v| v.as_str()));
                let pin = version.and_then(poetry_exact);
                out.push((Ecosystem::PyPI.normalize_name(name), pin));
            }
        }
    }
    let mut specs: Vec<&str> = Vec::new();
    if let Some(project) = doc.get("project") {
        if let Some(deps) = project.get("dependencies").and_then(|v| v.as_array()) {
            specs.extend(deps.iter().filter_map(|v| v.as_str()));
        }
        if let Some(opt) = project
            .get("optional-dependencies")
            .and_then(|v| v.as_table())
        {
            for list in opt.values().filter_map(|v| v.as_array()) {
                specs.extend(list.iter().filter_map(|v| v.as_str()));
            }
        }
    }
    if let Some(groups) = doc.get("dependency-groups").and_then(|v| v.as_table()) {
        for list in groups.values().filter_map(|v| v.as_array()) {
            specs.extend(list.iter().filter_map(|v| v.as_str()));
        }
    }
    out.extend(specs.into_iter().filter_map(pep508));
    Ok(out)
}

/// A Poetry constraint that pins one exact version (`"1.2.3"` or `"==1.2.3"`).
fn poetry_exact(c: &str) -> Option<String> {
    let c = c.trim();
    let v = c.strip_prefix("==").unwrap_or(c).trim();
    let exact = !v.is_empty()
        && v.chars().next().is_some_and(|ch| ch.is_ascii_digit())
        && !v.contains(['*', ',', '<', '>', '^', '~', '|', ' ']);
    exact.then(|| v.to_string())
}

fn poetry_lock(lock: &Path, direct: &HashSet<String>) -> Result<Vec<Package>> {
    let doc = parse_toml(lock)?;
    let mut out = Vec::new();
    for e in doc
        .get("package")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        let source_type = e
            .get("source")
            .and_then(|s| s.get("type"))
            .and_then(|v| v.as_str());
        if matches!(source_type, Some("git" | "directory" | "file" | "url")) {
            continue;
        }
        let (Some(name), Some(version)) = (
            e.get("name").and_then(|v| v.as_str()),
            e.get("version").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        let name = Ecosystem::PyPI.normalize_name(name);
        out.push(Package {
            direct: direct.contains(&name),
            name,
            version: version.to_string(),
            ecosystem: Ecosystem::PyPI,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pep508_variants() {
        assert_eq!(
            pep508("Django==4.2.1"),
            Some(("django".into(), Some("4.2.1".into())))
        );
        assert_eq!(
            pep508("requests[socks] == 2.31.0 ; python_version >= '3.8'"),
            Some(("requests".into(), Some("2.31.0".into())))
        );
        assert_eq!(pep508("flask>=2.0"), Some(("flask".into(), None)));
        assert_eq!(pep508("numpy==1.*"), Some(("numpy".into(), None)));
        assert_eq!(
            pep508("Zope.Interface===5.0"),
            Some(("zope-interface".into(), Some("5.0".into())))
        );
        assert_eq!(
            pep508("pkg @ https://example.com/pkg.whl"),
            Some(("pkg".into(), None))
        );
        assert_eq!(pep508("-r other.txt"), None);
        assert_eq!(pep508("https://example.com/x.whl"), None);
        assert_eq!(pep508("./local/pkg"), None);
        assert_eq!(pep508(""), None);
    }

    #[test]
    fn poetry_exact_pins() {
        assert_eq!(poetry_exact("1.2.3"), Some("1.2.3".into()));
        assert_eq!(poetry_exact("==1.2.3"), Some("1.2.3".into()));
        assert_eq!(poetry_exact("^1.2"), None);
        assert_eq!(poetry_exact(">=1,<2"), None);
        assert_eq!(poetry_exact("*"), None);
    }
}
