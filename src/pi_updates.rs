//! Updates for pi itself and its installed packages (extensions), checked
//! against the npm registry and applied only when the user asks, through
//! pi's own `pi update` commands.
//!
//! A pi update is judged against the app's support range first: a newer
//! minor is offered with an "untested" note, a new major is not offered
//! until an app release supports it.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::pi::{self, Installation, Version};

const PI_PACKAGE: &str = "@earendil-works/pi-coding-agent";

#[derive(Clone, Debug, PartialEq)]
pub struct PackageUpdate {
    /// As configured in pi, e.g. `npm:pi-subagents`; what `pi update` takes.
    pub source: String,
    pub name: String,
    pub current: String,
    pub latest: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PiCompatibility {
    /// Within the tested range.
    Supported,
    /// Same major, newer minor than the app was tested with.
    Untested,
    /// A new major: the app needs an update that supports it first.
    NeedsAppUpdate,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Updates {
    /// Newer pi on npm, if any.
    pub pi: Option<Version>,
    pub packages: Vec<PackageUpdate>,
}

pub fn compatibility(version: Version) -> PiCompatibility {
    if !pi::supports(version) {
        PiCompatibility::NeedsAppUpdate
    } else if pi::is_untested(version) {
        PiCompatibility::Untested
    } else {
        PiCompatibility::Supported
    }
}

/// Looks up what can be updated. Blocking (runs `pi list` and curl).
pub fn check(installation: &Installation) -> Updates {
    let pi = latest_version(PI_PACKAGE)
        .and_then(|latest| Version::find_in(&latest))
        .filter(|latest| *latest > installation.version);
    let packages = installed_packages(installation)
        .into_iter()
        .filter_map(|(source, path)| {
            let name = npm_name(&source)?;
            let current = package_version(&path)?;
            let latest = latest_version(&name)?;
            is_newer(&latest, &current).then_some(PackageUpdate {
                source,
                name,
                current,
                latest,
            })
        })
        .collect();
    Updates { pi, packages }
}

/// Runs `pi update <args>` and returns its output. Blocking.
pub fn run(installation: &Installation, args: &[&str]) -> Result<String, String> {
    let output = Command::new(&installation.executable)
        .arg("update")
        .args(args)
        .env(
            "PATH",
            pi::path_with_executable_dir(&installation.executable),
        )
        .output()
        .map_err(|error| format!("could not run pi: {error}"))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if output.status.success() {
        Ok(text)
    } else {
        Err(text
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("pi update failed")
            .to_string())
    }
}

/// `(source, install path)` pairs from `pi list`.
fn installed_packages(installation: &Installation) -> Vec<(String, PathBuf)> {
    let Ok(output) = Command::new(&installation.executable)
        .arg("list")
        .env(
            "PATH",
            pi::path_with_executable_dir(&installation.executable),
        )
        .output()
    else {
        return Vec::new();
    };
    parse_list(&String::from_utf8_lossy(&output.stdout))
}

fn parse_list(text: &str) -> Vec<(String, PathBuf)> {
    let mut packages = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let source = line.trim();
        let is_source = line.starts_with("  ") && !line.starts_with("    ") && !source.is_empty();
        if !is_source {
            continue;
        }
        if let Some(next) = lines.peek()
            && next.starts_with("    ")
        {
            packages.push((source.to_string(), PathBuf::from(next.trim())));
            lines.next();
        }
    }
    packages
}

/// The npm package name of an unpinned `npm:` source; pinned versions and
/// git sources are not offered (pi does not move them either).
fn npm_name(source: &str) -> Option<String> {
    let spec = source.strip_prefix("npm:")?;
    let (scope, rest) = match spec.strip_prefix('@') {
        Some(rest) => ("@", rest),
        None => ("", spec),
    };
    if rest.contains('@') {
        return None;
    }
    Some(format!("{scope}{rest}"))
}

fn package_version(path: &Path) -> Option<String> {
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(path.join("package.json")).ok()?).ok()?;
    manifest["version"].as_str().map(str::to_string)
}

fn latest_version(name: &str) -> Option<String> {
    let output = Command::new("curl")
        .args(["-fsSL", "--max-time", "15"])
        .arg(format!("https://registry.npmjs.org/{name}/latest"))
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let manifest: Value = serde_json::from_slice(&output.stdout).ok()?;
    manifest["version"].as_str().map(str::to_string)
}

fn is_newer(latest: &str, current: &str) -> bool {
    match (Version::find_in(latest), Version::find_in(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => latest != current,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pi_list_output() {
        let text = "User packages:\n  npm:pi-scraper\n    /home/u/.pi/agent/npm/node_modules/pi-scraper\n  npm:@gotgenes/pi-anthropic-auth\n    /home/u/.pi/agent/npm/node_modules/@gotgenes/pi-anthropic-auth\n";
        let packages = parse_list(text);
        assert_eq!(packages.len(), 2);
        assert_eq!(packages[1].0, "npm:@gotgenes/pi-anthropic-auth");
        assert!(packages[1].1.ends_with("@gotgenes/pi-anthropic-auth"));
    }

    #[test]
    fn only_unpinned_npm_sources_are_checked() {
        assert_eq!(
            npm_name("npm:pi-subagents").as_deref(),
            Some("pi-subagents")
        );
        assert_eq!(npm_name("npm:@scope/pkg").as_deref(), Some("@scope/pkg"));
        assert_eq!(npm_name("npm:pi-subagents@0.80.0"), None);
        assert_eq!(npm_name("npm:@scope/pkg@1.0.0"), None);
        assert_eq!(npm_name("git:github.com/a/b"), None);
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("0.85.0", "0.83.1"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        let version = |text| Version::find_in(text).unwrap();
        assert_eq!(compatibility(version("1.4.9")), PiCompatibility::Supported);
        assert_eq!(compatibility(version("1.5.0")), PiCompatibility::Untested);
        assert_eq!(
            compatibility(version("2.0.0")),
            PiCompatibility::NeedsAppUpdate
        );
    }
}
