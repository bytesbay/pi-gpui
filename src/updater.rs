//! Updates, as in Zed: check GitHub Releases in the background, download and
//! stage the new app, then offer "Restart to update" in the title bar.
//!
//! Each release carries `pi-compat.json` (from `pi-gpui --pi-compat`) with
//! the pi versions it supports. The updater offers the newest release that
//! supports the installed pi; pi itself is never updated by the app.
//!
//! Network and file work runs in background threads through macOS tools that
//! are always present (`curl`, `ditto`, `plutil`), so no HTTP stack is linked.
//! Updates only run from an app bundle; development builds never check.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use serde_json::Value;

use crate::pi::Version;

pub const REPOSITORY: &str = "bytesbay/pi-gpui";
const BUNDLE_ID: &str = "dev.pi-gpui.app";
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(15);
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Clone, Debug, PartialEq)]
pub enum UpdateStatus {
    Idle,
    Checking,
    Downloading(Version),
    /// Staged next to the running app; a restart swaps them.
    Ready(Version),
    UpToDate,
    Failed(String),
}

pub struct AutoUpdater {
    status: UpdateStatus,
    current: Version,
    /// The installed pi's version: releases that do not support it are skipped.
    pi: Option<Version>,
    /// The running `.app`, `None` for development builds.
    bundle: Option<PathBuf>,
    /// The result of a check the user asked for, shown once by the hub.
    manual_result: Option<UpdateStatus>,
    manual: bool,
    /// Version before this launch, when the app was just updated.
    updated_from: Option<String>,
    _poll: Task<()>,
    _check: Task<()>,
}

struct GlobalUpdater(Entity<AutoUpdater>);

impl Global for GlobalUpdater {}

impl AutoUpdater {
    pub fn init(cx: &mut App) {
        let updater = cx.new(|cx| {
            let bundle = running_bundle();
            let mut this = Self {
                status: UpdateStatus::Idle,
                current: pretend_version().unwrap_or_else(Version::app),
                pi: None,
                bundle,
                manual_result: None,
                manual: false,
                updated_from: remember_version(),
                _poll: Task::ready(()),
                _check: Task::ready(()),
            };
            if this.bundle.is_some() {
                this._poll = cx.spawn(async move |this: gpui::WeakEntity<Self>, cx| {
                    let mut delay = FIRST_CHECK_DELAY;
                    loop {
                        cx.background_executor().timer(delay).await;
                        if this
                            .update(cx, |this: &mut Self, cx| this.check(false, cx))
                            .is_err()
                        {
                            return;
                        }
                        delay = CHECK_INTERVAL;
                    }
                });
            }
            this
        });
        cx.set_global(GlobalUpdater(updater));
    }

    pub fn global(cx: &App) -> Entity<AutoUpdater> {
        cx.global::<GlobalUpdater>().0.clone()
    }

    pub fn status(&self) -> &UpdateStatus {
        &self.status
    }

    pub fn current_version(&self) -> Version {
        self.current
    }

    /// The result of a "Check for Updates" the user asked for, once.
    pub fn take_manual_result(&mut self) -> Option<UpdateStatus> {
        self.manual_result.take()
    }

    pub fn take_updated_from(&mut self) -> Option<String> {
        self.updated_from.take()
    }

    pub fn set_pi_version(&mut self, version: Version) {
        self.pi = Some(version);
    }

    pub fn check(&mut self, manual: bool, cx: &mut Context<Self>) {
        if matches!(
            self.status,
            UpdateStatus::Checking | UpdateStatus::Downloading(_)
        ) {
            return;
        }
        if let UpdateStatus::Ready(_) = self.status {
            if manual {
                self.manual_result = Some(self.status.clone());
                cx.notify();
            }
            return;
        }
        let Some(bundle) = self.bundle.clone() else {
            if manual {
                self.manual_result = Some(UpdateStatus::Failed(
                    "updates work in the app bundle (script/bundle.sh), not in a development build"
                        .to_string(),
                ));
                cx.notify();
            }
            return;
        };
        self.manual = manual;
        self.status = UpdateStatus::Checking;
        cx.notify();

        let current = self.current;
        let pi = self.pi;
        let lookup = cx.background_spawn(async move { latest_release(current, pi) });
        self._check = cx.spawn(async move |this, cx| {
            let found = lookup.await;
            let release = match found {
                Ok(Some(release)) => release,
                Ok(None) => {
                    this.update(cx, |this, cx| this.finish(UpdateStatus::UpToDate, cx))
                        .ok();
                    return;
                }
                Err(error) => {
                    this.update(cx, |this, cx| this.finish(UpdateStatus::Failed(error), cx))
                        .ok();
                    return;
                }
            };
            let version = release.version;
            this.update(cx, |this, cx| {
                this.status = UpdateStatus::Downloading(version);
                cx.notify();
            })
            .ok();
            let staged = cx
                .background_spawn(async move { stage(&release, &bundle) })
                .await;
            this.update(cx, |this, cx| {
                let status = match staged {
                    Ok(()) => UpdateStatus::Ready(version),
                    Err(error) => UpdateStatus::Failed(error),
                };
                this.finish(status, cx)
            })
            .ok();
        });
    }

    fn finish(&mut self, status: UpdateStatus, cx: &mut Context<Self>) {
        if let UpdateStatus::Failed(error) = &status {
            eprintln!("pi-gpui: update check failed: {error}");
        }
        if self.manual {
            self.manual_result = Some(status.clone());
            self.manual = false;
        }
        // A failed background check is retried later; it is not shown.
        self.status = match status {
            UpdateStatus::Failed(_) | UpdateStatus::UpToDate => UpdateStatus::Idle,
            status => status,
        };
        cx.notify();
    }

    /// Swaps the staged app in, relaunches it once this process has exited,
    /// and quits. The running process keeps working from the old files
    /// until it exits, as macOS allows.
    pub fn restart_to_update(&mut self, cx: &mut Context<Self>) {
        let Some(bundle) = self.bundle.clone() else {
            return;
        };
        match swap_in_staged(&bundle) {
            Ok(old) => {
                let script = format!(
                    "while kill -0 {pid} 2>/dev/null; do sleep 0.1; done; rm -rf {old}; open {app}",
                    pid = std::process::id(),
                    old = shell_quote(&old),
                    app = shell_quote(&bundle),
                );
                let spawned = Command::new("/bin/sh")
                    .arg("-c")
                    .arg(script)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
                if let Err(error) = spawned {
                    self.status = UpdateStatus::Idle;
                    self.manual_result = Some(UpdateStatus::Failed(format!(
                        "could not relaunch: {error}; open the app again to finish"
                    )));
                    cx.notify();
                    return;
                }
                cx.quit();
            }
            Err(error) => {
                self.status = UpdateStatus::Idle;
                self.manual_result = Some(UpdateStatus::Failed(error));
                cx.notify();
            }
        }
    }
}

struct Release {
    version: Version,
    url: String,
    /// `pi-compat.json`, when the release has one.
    compat_url: Option<String>,
}

/// How many newer releases are tried, newest first, to find one that
/// supports the installed pi.
const RELEASES_TRIED: usize = 5;

/// The newest release that is newer than `current` and supports `pi`.
fn latest_release(current: Version, pi: Option<Version>) -> Result<Option<Release>, String> {
    let url = format!("https://api.github.com/repos/{REPOSITORY}/releases?per_page=30");
    let releases: Value = serde_json::from_slice(&fetch(&url, current)?)
        .map_err(|error| format!("unexpected answer from GitHub: {error}"))?;
    for release in newer_releases(&releases, current, std::env::consts::ARCH)
        .into_iter()
        .take(RELEASES_TRIED)
    {
        let (Some(pi), Some(compat_url)) = (pi, &release.compat_url) else {
            return Ok(Some(release));
        };
        let compat: Value = match fetch(compat_url, current)
            .and_then(|body| serde_json::from_slice(&body).map_err(|error| error.to_string()))
        {
            Ok(compat) => compat,
            Err(error) => {
                eprintln!("pi-gpui: skipping {} ({error})", release.version);
                continue;
            }
        };
        if release_supports(&compat, pi) {
            return Ok(Some(release));
        }
    }
    Ok(None)
}

fn fetch(url: &str, current: Version) -> Result<Vec<u8>, String> {
    let output = Command::new("curl")
        .args(["-fsSL", "--max-time", "30"])
        .args(["-H", "Accept: application/vnd.github+json"])
        .args(["-H", &format!("User-Agent: pi-gpui/{current}")])
        .arg(url)
        .output()
        .map_err(|error| format!("could not run curl: {error}"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(format!(
            "GitHub did not answer: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// Published releases newer than `current` with an app for `arch`, newest first.
fn newer_releases(releases: &Value, current: Version, arch: &str) -> Vec<Release> {
    let suffix = format!("-{arch}.zip");
    let mut found: Vec<Release> = releases
        .as_array()
        .into_iter()
        .flatten()
        .filter(|release| {
            release["draft"].as_bool() != Some(true)
                && release["prerelease"].as_bool() != Some(true)
        })
        .filter_map(|release| {
            let version = Version::find_in(release["tag_name"].as_str()?)?;
            if version <= current {
                return None;
            }
            let assets = release["assets"].as_array()?;
            let asset_url = |wanted: &dyn Fn(&str) -> bool| {
                assets
                    .iter()
                    .find(|asset| asset["name"].as_str().is_some_and(wanted))
                    .and_then(|asset| asset["browser_download_url"].as_str())
                    .map(str::to_string)
            };
            Some(Release {
                version,
                url: asset_url(&|name| name.ends_with(&suffix))?,
                compat_url: asset_url(&|name| name == "pi-compat.json"),
            })
        })
        .collect();
    found.sort_by(|left, right| right.version.cmp(&left.version));
    found
}

/// Whether a release's `pi-compat.json` covers `pi`: at least `min`, same major.
fn release_supports(compat: &Value, pi: Version) -> bool {
    let min = compat["pi"]["min"].as_str().and_then(Version::find_in);
    let major = compat["pi"]["major"].as_u64();
    match (min, major) {
        (Some(min), Some(major)) => pi >= min && pi.major == major,
        // An unreadable file does not block updates.
        _ => true,
    }
}

/// Downloads and unpacks the release, checks it, and copies it next to the
/// running app (same volume, so the restart is two renames).
fn stage(release: &Release, bundle: &Path) -> Result<(), String> {
    let cache = crate::sessions::home_directory()
        .ok_or("no home directory")?
        .join("Library/Caches/pi-gpui/updates")
        .join(release.version.to_string());
    let _ = std::fs::remove_dir_all(&cache);
    std::fs::create_dir_all(&cache).map_err(|error| error.to_string())?;
    let archive = cache.join("update.zip");
    run(Command::new("curl")
        .args(["-fsSL", "--max-time", "900", "-o"])
        .arg(&archive)
        .arg(&release.url))?;
    let unpacked = cache.join("unpacked");
    run(Command::new("ditto")
        .arg("-x")
        .arg("-k")
        .arg(&archive)
        .arg(&unpacked))?;
    let app = std::fs::read_dir(&unpacked)
        .map_err(|error| error.to_string())?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|extension| extension == "app"))
        .ok_or("the download has no app inside")?;

    let plist = app.join("Contents/Info.plist");
    let identifier = plist_value(&plist, "CFBundleIdentifier")?;
    let version = plist_value(&plist, "CFBundleShortVersionString")?;
    if identifier != BUNDLE_ID || Version::find_in(&version) != Some(release.version) {
        return Err(format!(
            "the download is not Pi GPUI {} ({identifier} {version})",
            release.version
        ));
    }

    let staged = staged_path(bundle)?;
    let _ = std::fs::remove_dir_all(&staged);
    run(Command::new("ditto").arg(&app).arg(&staged)).map_err(|error| {
        format!(
            "could not write next to the app in {} ({error}); move the app to a folder you can write to",
            bundle.parent().map_or_else(String::new, |parent| parent.display().to_string())
        )
    })?;
    let _ = std::fs::remove_dir_all(&cache);
    Ok(())
}

/// Moves the running app aside and the staged one into its place. Returns
/// where the old app went (removed after the relaunch).
fn swap_in_staged(bundle: &Path) -> Result<PathBuf, String> {
    let staged = staged_path(bundle)?;
    if !staged.is_dir() {
        return Err("the downloaded update is gone; it will be downloaded again".to_string());
    }
    let old = bundle.with_file_name(format!(
        ".{}.old",
        bundle
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("app")
    ));
    let _ = std::fs::remove_dir_all(&old);
    std::fs::rename(bundle, &old).map_err(|error| format!("could not replace the app: {error}"))?;
    if let Err(error) = std::fs::rename(&staged, bundle) {
        let _ = std::fs::rename(&old, bundle);
        return Err(format!("could not install the update: {error}"));
    }
    Ok(old)
}

fn staged_path(bundle: &Path) -> Result<PathBuf, String> {
    let name = bundle
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("the app has no name")?;
    Ok(bundle.with_file_name(format!(".{name}.update")))
}

fn plist_value(plist: &Path, key: &str) -> Result<String, String> {
    let output = Command::new("plutil")
        .args(["-extract", key, "raw", "-o", "-"])
        .arg(plist)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!("the download has no {key}"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn run(command: &mut Command) -> Result<(), String> {
    let output = command.output().map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// `…/Pi GPUI.app` when running from a bundle.
fn running_bundle() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?.canonicalize().ok()?;
    let contents = executable.parent()?.parent()?;
    let bundle = contents.parent()?;
    (contents.file_name()? == "Contents" && bundle.extension()? == "app")
        .then(|| bundle.to_path_buf())
}

/// Testing aid: `PI_GPUI_PRETEND_VERSION=1.4.0` makes the app believe it is
/// that version, so the update flow can be tried against a real release.
fn pretend_version() -> Option<Version> {
    Version::find_in(&std::env::var("PI_GPUI_PRETEND_VERSION").ok()?)
}

/// Stores this launch's version; returns the previous one if it differs.
fn remember_version() -> Option<String> {
    let path =
        crate::sessions::home_directory()?.join("Library/Application Support/pi-gpui/last-version");
    let current = env!("CARGO_PKG_VERSION");
    let previous = std::fs::read_to_string(&path).ok();
    if previous.as_deref().map(str::trim) != Some(current) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, current);
    }
    previous
        .map(|previous| previous.trim().to_string())
        .filter(|previous| previous != current)
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn version(text: &str) -> Version {
        Version::find_in(text).unwrap()
    }

    fn releases() -> Value {
        let release = |tag: &str, prerelease: bool| {
            json!({
                "tag_name": tag,
                "draft": false,
                "prerelease": prerelease,
                "assets": [
                    {
                        "name": format!("Pi-GPUI-{}-aarch64.zip", &tag[1..]),
                        "browser_download_url": format!("https://example.com/{tag}.zip"),
                    },
                    {
                        "name": "pi-compat.json",
                        "browser_download_url": format!("https://example.com/{tag}.json"),
                    },
                ],
            })
        };
        json!([
            release("v0.3.0", true),
            release("v0.2.1", false),
            release("v0.1.0", false),
            release("v0.2.0", false),
        ])
    }

    #[test]
    fn newer_published_releases_newest_first() {
        let found = newer_releases(&releases(), version("0.1.0"), "aarch64");
        let versions: Vec<String> = found
            .iter()
            .map(|release| release.version.to_string())
            .collect();
        // 0.3.0 is a prerelease.
        assert_eq!(versions, ["0.2.1", "0.2.0"]);
        assert_eq!(
            found[0].compat_url.as_deref(),
            Some("https://example.com/v0.2.1.json")
        );
        assert!(newer_releases(&releases(), version("0.1.0"), "x86_64").is_empty());
        assert!(newer_releases(&releases(), version("0.2.1"), "aarch64").is_empty());
    }

    #[test]
    fn compat_file_decides_whether_a_release_fits_pi() {
        let compat =
            json!({ "app": "0.2.1", "pi": { "min": "1.5.0", "major": 1, "tested": "1.5" } });
        assert!(release_supports(&compat, version("1.5.2")));
        assert!(release_supports(&compat, version("1.7.0")));
        assert!(!release_supports(&compat, version("1.4.9")));
        assert!(!release_supports(&compat, version("2.0.0")));
        assert!(release_supports(&json!({}), version("1.4.0")));
    }
}
