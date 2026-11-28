//! Downloading and starting the KeptCraft installer.
//!
//! KeptCraft is the image editor Kept is built around, so the setup wizard and
//! Settings can fetch it instead of sending the user to a browser. The release
//! list is read from the GitHub API, the newest build for this machine is
//! downloaded over HTTPS and its SHA-256 is checked against the digest GitHub
//! publishes before anything is run; a build with no published digest is refused
//! rather than trusted.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::{AppError, Result};

/// Releases are published as pre-releases, so `releases/latest` answers 404 and
/// the list has to be read instead.
const RELEASES: &str = "https://api.github.com/repos/jamesbuckley-source/KeptCraft/releases?per_page=10";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Generous for an installer, small enough that a wrong URL cannot fill the disk.
const MAX_INSTALLER_BYTES: usize = 300 * 1024 * 1024;

/// A release and the one build inside it that suits this machine.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    /// The tag it was published under, such as `v0.3.0-beta.2`.
    pub version: String,
    /// The file that will be downloaded, with its digest.
    pub asset: Asset,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// Lower-case hex SHA-256, as published by the release.
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Os {
    Windows,
    Macos,
    Linux,
}

impl Os {
    fn here() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::Macos
        } else {
            Os::Linux
        }
    }
}

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent("Kept clipboard manager")
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| AppError::msg(format!("could not prepare the download: {e}")))
}

/// The newest published KeptCraft build for this machine.
pub async fn latest() -> Result<Release> {
    let response = client()?
        .get(RELEASES)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| AppError::msg(format!("could not reach GitHub: {e}")))?;
    let status = response.status();
    if !status.is_success() {
        return Err(AppError::msg(format!(
            "GitHub answered {status} instead of a release list"
        )));
    }
    let body: Value = response
        .json()
        .await
        .map_err(|e| AppError::msg(format!("could not read the release list: {e}")))?;
    pick_release(&body)
}

/// Chooses the release and build from the API's answer.
///
/// Drafts are ignored — they are not published — and the list arrives newest
/// first, so the first usable entry wins. Only `keptcraft-…` files are eligible:
/// the repository's older releases shipped a differently named app.
fn pick_release(list: &Value) -> Result<Release> {
    let releases = list
        .as_array()
        .ok_or_else(|| AppError::msg("GitHub did not return a list of releases"))?;
    let mut seen_any = false;
    for release in releases {
        if release["draft"].as_bool().unwrap_or(false) {
            continue;
        }
        seen_any = true;
        let Some(version) = release["tag_name"].as_str() else {
            continue;
        };
        let Some(assets) = release["assets"].as_array() else {
            continue;
        };
        if let Some(asset) = select_asset(assets, Os::here(), std::env::consts::ARCH) {
            return Ok(Release {
                version: version.to_string(),
                asset,
            });
        }
    }
    Err(AppError::msg(if seen_any {
        format!(
            "the newest KeptCraft release has no installer for {} {}. Download one from the release page instead.",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    } else {
        "KeptCraft has no published releases yet".to_string()
    }))
}

/// The build for `os` and `arch`, or `None` when the release does not carry one.
///
/// `arch` is the Rust target name (`x86_64`, `aarch64`), while assets use the
/// vendor spelling (`x64`, `arm64`), so both are accepted.
fn select_asset(assets: &[Value], os: Os, arch: &str) -> Option<Asset> {
    let arch_names: &[&str] = match arch {
        "x86_64" => &["x86_64", "x64", "amd64"],
        "aarch64" => &["aarch64", "arm64"],
        _ => &[arch],
    };
    // Ordered by preference: a package the desktop can install first, a portable
    // archive last. Installers are what the user asked for; an archive still
    // beats failing.
    let preferred: &[&str] = match os {
        Os::Windows => &[".msi", ".zip"],
        Os::Macos => &[".dmg"],
        Os::Linux => &[".appimage", ".deb", ".rpm", ".tar.gz"],
    };

    let mut best: Option<(usize, Asset)> = None;
    for asset in assets {
        let name = asset["name"].as_str().unwrap_or_default();
        let lower = name.to_ascii_lowercase();
        if !lower.starts_with("keptcraft-") {
            continue;
        }
        let target_ok = match os {
            Os::Windows => lower.contains("windows"),
            Os::Macos => lower.contains("macos") || lower.contains("darwin"),
            Os::Linux => lower.contains("linux"),
        };
        if !target_ok {
            continue;
        }
        // macOS publishes one universal build, so the arch is not in its name.
        if os != Os::Macos && !arch_names.iter().any(|token| lower.contains(token)) {
            continue;
        }
        let Some(rank) = preferred.iter().position(|ext| lower.ends_with(ext)) else {
            continue;
        };
        let Some(url) = asset["browser_download_url"].as_str() else {
            continue;
        };
        let Some(sha256) = published_digest(asset) else {
            // Without a digest there is nothing to check the download against.
            continue;
        };
        let candidate = Asset {
            name: name.to_string(),
            url: url.to_string(),
            size: asset["size"].as_u64().unwrap_or(0),
            sha256,
        };
        if best.as_ref().is_none_or(|(rank_so_far, _)| rank < *rank_so_far) {
            best = Some((rank, candidate));
        }
    }
    best.map(|(_, asset)| asset)
}

/// The asset's SHA-256 as GitHub publishes it (`sha256:<hex>`).
fn published_digest(asset: &Value) -> Option<String> {
    let digest = asset["digest"].as_str()?.trim();
    let hex = digest.strip_prefix("sha256:").unwrap_or(digest);
    let hex = hex.to_ascii_lowercase();
    let is_hash = hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit());
    is_hash.then_some(hex)
}

/// Downloads an installer, refusing anything the digest does not match.
pub async fn fetch(asset: &Asset) -> Result<Vec<u8>> {
    let response = client()?
        .get(&asset.url)
        .send()
        .await
        .map_err(|e| AppError::msg(format!("could not download KeptCraft: {e}")))?;
    let status = response.status();
    if !status.is_success() {
        return Err(AppError::msg(format!(
            "GitHub answered {status} instead of the installer"
        )));
    }
    if let Some(length) = response.content_length() {
        if length as usize > MAX_INSTALLER_BYTES {
            return Err(AppError::msg(format!(
                "that installer is {} MB, which is larger than Kept will download",
                length / (1024 * 1024)
            )));
        }
    }
    let mut bytes = Vec::new();
    let mut response = response;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| AppError::msg(format!("the download stopped early: {e}")))?
    {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > MAX_INSTALLER_BYTES {
            return Err(AppError::msg(
                "the download was larger than Kept will accept",
            ));
        }
    }
    verify(asset, &bytes)?;
    Ok(bytes)
}

/// Refuses a download that does not hash to what the release published.
fn verify(asset: &Asset, bytes: &[u8]) -> Result<()> {
    let digest = hex::encode(Sha256::digest(bytes));
    if digest != asset.sha256 {
        return Err(AppError::msg(format!(
            "the downloaded {} did not match its published SHA-256, so it was discarded",
            asset.name
        )));
    }
    Ok(())
}

/// Where the installer is written.
///
/// A package the user has to install themselves (`.deb`, `.rpm`, `.AppImage`)
/// goes to Downloads so it is still there afterwards; the others are handed to
/// the system installer immediately and live in the temporary folder.
fn save_dir(asset: &Asset) -> PathBuf {
    let lower = asset.name.to_ascii_lowercase();
    let self_installed = lower.ends_with(".appimage")
        || lower.ends_with(".deb")
        || lower.ends_with(".rpm")
        || lower.ends_with(".zip");
    if self_installed {
        if let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) {
            let downloads = Path::new(&home).join("Downloads");
            if downloads.is_dir() {
                return downloads;
            }
        }
    }
    std::env::temp_dir().join("kept-installers")
}

/// Writes the installer and hands it to the system, returning where it landed.
fn save_and_start(asset: &Asset, bytes: &[u8]) -> Result<PathBuf> {
    let dir = save_dir(asset);
    std::fs::create_dir_all(&dir)
        .map_err(|e| AppError::msg(format!("could not prepare {}: {e}", dir.display())))?;
    let path = dir.join(&asset.name);
    std::fs::write(&path, bytes)
        .map_err(|e| AppError::msg(format!("could not save the installer: {e}")))?;
    if let Err(e) = start(&path) {
        return Err(AppError::msg(format!(
            "the installer was saved to {}, but could not be started: {e}",
            path.display()
        )));
    }
    Ok(path)
}

/// Begins the install: the system's own installer for a package, the file
/// itself for an AppImage.
fn start(path: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        // The MSI asks for elevation itself, so this only has to hand it over.
        std::process::Command::new("msiexec")
            .arg("/i")
            .arg(path)
            .spawn()
            .map_err(|e| AppError::msg(format!("msiexec could not be started: {e}")))?;
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .map_err(|e| AppError::msg(format!("open could not be started: {e}")))?;
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        use std::os::unix::fs::PermissionsExt;
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("appimage"))
        {
            // An AppImage is the program: make it runnable and start it.
            let mut permissions = std::fs::metadata(path)?.permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(path, permissions)?;
            std::process::Command::new(path)
                .spawn()
                .map_err(|e| AppError::msg(format!("the AppImage could not be started: {e}")))?;
            return Ok(());
        }
        // A .deb or .rpm goes to the desktop's own installer rather than to a
        // package manager: installing it needs a password Kept should not ask for.
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map_err(|e| AppError::msg(format!("xdg-open could not be started: {e}")))?;
        Ok(())
    }
}

/// Downloads the newest KeptCraft and starts its installer, naming what happened.
pub async fn install() -> Result<String> {
    let release = latest().await?;
    let bytes = fetch(&release.asset).await?;
    let path = save_and_start(&release.asset, &bytes)?;
    Ok(format!(
        "KeptCraft {} downloaded: {} is saved to {} and its installer has started.",
        release.version,
        release.asset.name,
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assets(names: &[&str]) -> Vec<Value> {
        names
            .iter()
            .map(|name| {
                serde_json::json!({
                    "name": name,
                    "browser_download_url": format!("https://example.test/{name}"),
                    "size": 1234,
                    "digest": format!("sha256:{}", "a".repeat(64)),
                })
            })
            .collect()
    }

    const ALL: &[&str] = &[
        "keptcraft-0.3.0-beta.2-linux-aarch64.AppImage",
        "keptcraft-0.3.0-beta.2-linux-aarch64.deb",
        "keptcraft-0.3.0-beta.2-linux-x86_64.AppImage",
        "keptcraft-0.3.0-beta.2-linux-x86_64.deb",
        "keptcraft-0.3.0-beta.2-linux-x86_64.tar.gz",
        "keptcraft-0.3.0-beta.2-macos-universal.dmg",
        "keptcraft-0.3.0-beta.2-windows-arm64.msi",
        "keptcraft-0.3.0-beta.2-windows-x64.msi",
        "keptcraft-0.3.0-beta.2-windows-x64.zip",
        "SHA256SUMS",
    ];

    #[test]
    fn each_platform_gets_the_installer_it_can_run() {
        let list = assets(ALL);
        assert_eq!(
            select_asset(&list, Os::Windows, "x86_64").map(|a| a.name),
            Some("keptcraft-0.3.0-beta.2-windows-x64.msi".to_string())
        );
        assert_eq!(
            select_asset(&list, Os::Windows, "aarch64").map(|a| a.name),
            Some("keptcraft-0.3.0-beta.2-windows-arm64.msi".to_string())
        );
        assert_eq!(
            select_asset(&list, Os::Macos, "aarch64").map(|a| a.name),
            Some("keptcraft-0.3.0-beta.2-macos-universal.dmg".to_string())
        );
        assert_eq!(
            select_asset(&list, Os::Linux, "x86_64").map(|a| a.name),
            Some("keptcraft-0.3.0-beta.2-linux-x86_64.AppImage".to_string())
        );
        assert_eq!(
            select_asset(&list, Os::Linux, "aarch64").map(|a| a.name),
            Some("keptcraft-0.3.0-beta.2-linux-aarch64.AppImage".to_string())
        );
    }

    #[test]
    fn the_older_app_name_is_not_installed() {
        let list = assets(&[
            "shotcraft4kept-0.2.0-beta.1-windows-x64.msi",
            "keptcraft-0.3.0-beta.2-windows-x64.msi",
        ]);
        assert_eq!(
            select_asset(&list, Os::Windows, "x86_64").map(|a| a.name),
            Some("keptcraft-0.3.0-beta.2-windows-x64.msi".to_string())
        );
        let only_old = assets(&["shotcraft4kept-0.2.0-beta.1-windows-x64.msi"]);
        assert!(select_asset(&only_old, Os::Windows, "x86_64").is_none());
    }

    #[test]
    fn a_build_without_a_digest_is_never_offered() {
        let mut list = assets(&["keptcraft-0.3.0-beta.2-windows-x64.msi"]);
        list[0]["digest"] = Value::Null;
        assert!(select_asset(&list, Os::Windows, "x86_64").is_none());
        list[0]["digest"] = serde_json::json!("not-a-digest");
        assert!(select_asset(&list, Os::Windows, "x86_64").is_none());
    }

    #[test]
    fn drafts_are_skipped_and_the_newest_usable_release_wins() {
        let list = serde_json::json!([
            {
                "tag_name": "v0.4.0",
                "draft": true,
                "assets": assets(&["keptcraft-0.4.0-windows-x64.msi"]),
            },
            {
                "tag_name": "v0.3.0-beta.2",
                "draft": false,
                "assets": assets(&["keptcraft-0.3.0-beta.2-windows-x64.msi"]),
            },
        ]);
        let release = pick_release(&list).expect("a release");
        assert_eq!(release.version, "v0.3.0-beta.2");
        assert_eq!(release.asset.sha256, "a".repeat(64));
    }

    #[test]
    fn a_release_with_no_build_for_this_machine_says_so() {
        let list = serde_json::json!([{
            "tag_name": "v0.3.0",
            "draft": false,
            "assets": assets(&["keptcraft-0.3.0-linux-x86_64.AppImage"]),
        }]);
        let error = pick_release(&list).expect_err("nothing for Windows");
        assert!(error.to_string().contains("no installer for"));
    }

    #[test]
    fn a_download_that_does_not_match_its_digest_is_refused() {
        let asset = Asset {
            name: "keptcraft.msi".to_string(),
            url: "https://example.test/keptcraft.msi".to_string(),
            size: 4,
            sha256: "b".repeat(64),
        };
        assert!(verify(&asset, b"bytes").is_err());
        let right = Asset {
            sha256: hex::encode(Sha256::digest(b"bytes")),
            ..asset
        };
        assert!(verify(&right, b"bytes").is_ok());
    }
}
