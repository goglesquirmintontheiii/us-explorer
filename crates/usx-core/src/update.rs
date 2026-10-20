//! Self-update from GitHub releases.
//!
//! Each release carries `US_Explorer-<ver>-x86_64.AppImage` and a matching `.sha256`.
//! When the game runs as an AppImage (`$APPIMAGE` is set), [`install`] downloads the
//! new file next to the current one, verifies it, swaps it in and the game restarts.

use crate::net::{self, NetError};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Release {
    pub version: semver::Version,
    pub tag: String,
    pub name: String,
    pub notes: String,
    pub url: String,
    pub appimage_url: Option<String>,
    pub appimage_size: u64,
    pub sha256_url: Option<String>,
}

#[derive(serde::Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

#[derive(serde::Deserialize)]
struct GhRelease {
    tag_name: String,
    name: Option<String>,
    body: Option<String>,
    html_url: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<GhAsset>,
}

pub fn current() -> semver::Version {
    semver::Version::parse(crate::VERSION).unwrap_or(semver::Version::new(0, 0, 0))
}

/// The newest published release, if it is newer than this build.
pub fn check() -> Result<Option<Release>, NetError> {
    let url = format!(
        "{}/repos/{}/releases?per_page=10",
        crate::sources::GITHUB_API,
        crate::REPO
    );
    let list: Vec<GhRelease> = net::get_json(&url)?;
    let best = list
        .into_iter()
        .filter(|r| !r.draft && !r.prerelease)
        .filter_map(|r| {
            semver::Version::parse(r.tag_name.trim_start_matches('v'))
                .ok()
                .map(|v| (v, r))
        })
        .max_by(|a, b| a.0.cmp(&b.0));
    let Some((version, r)) = best else {
        return Ok(None);
    };
    if version <= current() {
        return Ok(None);
    }
    let app = r
        .assets
        .iter()
        .find(|a| a.name.ends_with("x86_64.AppImage"));
    let sha = app.and_then(|a| {
        r.assets
            .iter()
            .find(|s| s.name == format!("{}.sha256", a.name))
    });
    Ok(Some(Release {
        version,
        tag: r.tag_name.clone(),
        name: r.name.unwrap_or(r.tag_name),
        notes: r.body.unwrap_or_default(),
        url: r.html_url,
        appimage_url: app.map(|a| a.browser_download_url.clone()),
        appimage_size: app.map(|a| a.size).unwrap_or(0),
        sha256_url: sha.map(|a| a.browser_download_url.clone()),
    }))
}

/// Path of the running AppImage, when the game was launched as one.
pub fn appimage_path() -> Option<PathBuf> {
    std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
}

/// Whether one-click install is possible on this platform/launch.
pub fn can_self_install() -> bool {
    cfg!(target_os = "linux") && appimage_path().is_some()
}

/// Download, verify and swap in the new AppImage. Returns the path to relaunch.
pub fn install(rel: &Release, progress: impl FnMut(u64, Option<u64>)) -> Result<PathBuf, String> {
    let target = appimage_path()
        .ok_or("Not running from an AppImage; download the new version from the release page.")?;
    let url = rel
        .appimage_url
        .as_ref()
        .ok_or("This release has no Linux AppImage.")?;
    let data = net::get_with(url, "application/octet-stream", progress)
        .map_err(|e| format!("Download failed: {e}"))?;
    if let Some(su) = &rel.sha256_url {
        let want = net::get(su).map_err(|e| format!("Checksum download failed: {e}"))?;
        let want = String::from_utf8_lossy(&want)
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let got = Sha256::digest(&data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        if want != got {
            return Err("The download was corrupted (checksum mismatch). Try again.".into());
        }
    }
    let tmp = target.with_extension("new");
    std::fs::write(&tmp, &data).map_err(|e| format!("Couldn't write the update: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
    }
    // Renaming over a running AppImage is safe on Linux: the old inode stays mounted.
    std::fs::rename(&tmp, &target).map_err(|e| format!("Couldn't replace the game file: {e}"))?;
    Ok(target)
}

/// Start the given executable and exit this process.
pub fn relaunch(path: &std::path::Path) -> ! {
    let args: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with("--screenshot"))
        .collect();
    let _ = std::process::Command::new(path)
        .args(args)
        .env_remove("APPIMAGE")
        .spawn();
    std::process::exit(0);
}
