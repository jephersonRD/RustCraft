use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::blocking::Client;
use serde::{Deserialize, Deserializer, Serialize};

use crate::core::archive;
use crate::core::download;
use crate::core::error::{Error, Result};
use crate::core::hash::Checksum;
use crate::core::paths::AppPaths;

pub const REPO: &str = "horizon-rust/horizon";
pub const API_BASE: &str = "https://api.github.com/repos/horizon-rust/horizon";
pub const DEFAULT_TAG: &str = "nightly";
pub const LAUNCHER_REPO: &str = "jephersonRD/RustCraft";
pub const LAUNCHER_API: &str = "https://api.github.com/repos/jephersonRD/RustCraft";
const INSTALL_META: &str = "install.json";
const CHECKSUMS_ASSET: &str = "SHA256SUMS";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    LinuxX86_64,
    WindowsX64,
    WindowsArm64,
    MacosArm64,
    MacosX64,
}

impl Platform {
    pub fn current() -> Result<Self> {
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("linux", "x86_64") => Ok(Self::LinuxX86_64),
            ("windows", "x86_64") => Ok(Self::WindowsX64),
            ("windows", "aarch64") => Ok(Self::WindowsArm64),
            ("macos", "aarch64") => Ok(Self::MacosArm64),
            ("macos", "x86_64") => Ok(Self::MacosX64),
            (os, arch) => Err(Error::UnsupportedPlatform(format!("{os}-{arch}"))),
        }
    }

    pub fn asset_name(self) -> &'static str {
        match self {
            Self::LinuxX86_64 => "horizon-linux-x86_64.tar.gz",
            Self::WindowsX64 => "horizon-windows-x64.zip",
            Self::WindowsArm64 => "horizon-windows-arm64.zip",
            Self::MacosArm64 => "horizon-macos-arm64.zip",
            Self::MacosX64 => "horizon-macos-x64.zip",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default, deserialize_with = "null_string")]
    pub name: String,
    #[serde(default, deserialize_with = "null_string")]
    pub body: String,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

/// GitHub returns `null` for some text fields on older releases.
fn null_string<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Clone, Debug, Deserialize)]
pub struct Asset {
    pub name: String,
    pub size: u64,
    pub browser_download_url: String,
}

impl Release {
    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|asset| asset.name == name)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstallMeta {
    pub tag: String,
    pub asset: String,
    pub sha256: String,
    pub source_url: String,
    pub installed_at: u64,
}

#[derive(Clone, Debug)]
pub struct InstalledVersion {
    pub meta: InstallMeta,
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallEvent {
    Message(String),
    Download { downloaded: u64, total: Option<u64> },
}

#[derive(Clone, Debug)]
pub struct InstallResult {
    pub meta: InstallMeta,
    pub path: PathBuf,
    pub reused: bool,
}

pub fn fetch_release(client: &Client, tag: &str) -> Result<Release> {
    validate_tag(tag)?;
    let url = format!("{API_BASE}/releases/tags/{tag}");
    download::fetch_json(client, &url)
}

/// Newest published RustCraft launcher release, if any.
pub fn latest_launcher_release(client: &Client) -> Result<Release> {
    let url = format!("{LAUNCHER_API}/releases/latest");
    download::fetch_json(client, &url)
}

/// Asset name of the launcher binary for the current platform.
pub fn launcher_asset_name() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("rustcraft-linux-x86_64"),
        ("linux", "aarch64") => Some("rustcraft-linux-aarch64"),
        ("macos", "aarch64") => Some("rustcraft-macos-arm64"),
        ("macos", "x86_64") => Some("rustcraft-macos-x86_64"),
        ("windows", "x86_64") => Some("rustcraft-windows-x64.exe"),
        ("windows", "aarch64") => Some("rustcraft-windows-arm64.exe"),
        _ => None,
    }
}

/// Compare dotted versions, accepting an optional leading `v` and ignoring
/// trailing non-numeric parts such as `-beta`.
pub fn version_is_newer(current: &str, candidate: &str) -> bool {
    fn parts(version: &str) -> Vec<u32> {
        version
            .trim()
            .trim_start_matches('v')
            .split('.')
            .map(|part| {
                part.chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0)
            })
            .collect()
    }
    let current = parts(current);
    let candidate = parts(candidate);
    let length = current.len().max(candidate.len());
    for index in 0..length {
        let old = current.get(index).copied().unwrap_or(0);
        let new = candidate.get(index).copied().unwrap_or(0);
        if new != old {
            return new > old;
        }
    }
    false
}

/// The game's releases, newest first. Launcher releases (`launcher@...`) are skipped.
///
/// GitHub returns at most 100 entries per page and launcher releases are frequent,
/// so several pages are walked until `limit` game releases are collected.
pub fn list_releases(client: &Client, limit: usize) -> Result<Vec<Release>> {
    let mut releases = Vec::new();
    let mut page = 1;
    while releases.len() < limit && page <= 10 {
        let url = format!(
            "{API_BASE}/releases?per_page=100&page={page}"
        );
        let batch: Vec<Release> = download::fetch_json(client, &url)?;
        if batch.is_empty() {
            break;
        }
        let received = batch.len();
        releases.extend(
            batch
                .into_iter()
                .filter(|release| is_game_tag(&release.tag_name)),
        );
        if received < 100 {
            break;
        }
        page += 1;
    }
    releases.truncate(limit);
    Ok(releases)
}

/// Delete an installed Horizon version and its cached archive.
pub fn remove_installed(paths: &AppPaths, tag: &str) -> Result<()> {
    validate_tag(tag)?;
    let version_dir = paths.version_dir(tag);
    let meta = read_install_meta(&version_dir);
    if version_dir.exists() {
        fs::remove_dir_all(&version_dir).map_err(|e| {
            Error::io(
                format!("could not remove {}", version_dir.display()),
                e,
            )
        })?;
    }
    if let Some(meta) = meta {
        let _ = fs::remove_file(paths.downloads_cache_dir().join(meta.asset));
    }
    Ok(())
}

pub fn install_release(
    client: &Client,
    paths: &AppPaths,
    release: &Release,
    platform: Platform,
    cancel: &AtomicBool,
    mut progress: impl FnMut(InstallEvent),
) -> Result<InstallResult> {
    validate_tag(&release.tag_name)?;

    let asset_name = platform.asset_name();
    let asset = release
        .asset(asset_name)
        .ok_or_else(|| Error::MissingAsset(asset_name.to_owned()))?;

    let sums_asset = release
        .asset(CHECKSUMS_ASSET)
        .ok_or_else(|| Error::MissingAsset(CHECKSUMS_ASSET.to_owned()))?;
    let sums_text = download::fetch_text(client, &sums_asset.browser_download_url)?;
    let sums = parse_sha256sums(&sums_text);
    let expected = sums
        .get(asset_name)
        .cloned()
        .ok_or_else(|| Error::MissingChecksum(asset_name.to_owned()))?;

    let version_dir = paths.version_dir(&release.tag_name);
    if let Some(meta) = read_install_meta(&version_dir) {
        if meta.asset == asset_name
            && meta.sha256.eq_ignore_ascii_case(&expected)
            && has_horizon_binaries(&version_dir)
        {
            return Ok(InstallResult {
                meta,
                path: version_dir,
                reused: true,
            });
        }
    }

    progress(InstallEvent::Message(format!(
        "downloading {} ({:.1} MB)",
        asset_name,
        asset.size as f64 / 1_000_000.0
    )));

    let archive_path = paths.downloads_cache_dir().join(asset_name);
    let checksum = Checksum::Sha256(expected.clone());
    download::download(
        client,
        &asset.browser_download_url,
        &archive_path,
        Some(&checksum),
        cancel,
        |downloaded, total| {
            progress(InstallEvent::Download { downloaded, total });
        },
    )?;

    progress(InstallEvent::Message(format!(
        "installing {}",
        release.tag_name
    )));
    archive::extract_flat(&archive_path, &version_dir)?;

    if !has_horizon_binaries(&version_dir) {
        let _ = fs::remove_dir_all(&version_dir);
        return Err(Error::Install(format!(
            "{} does not contain horizon-client and horizon-prepare",
            release.tag_name
        )));
    }
    make_executable(&version_dir)?;

    let meta = InstallMeta {
        tag: release.tag_name.clone(),
        asset: asset_name.to_owned(),
        sha256: expected,
        source_url: asset.browser_download_url.clone(),
        installed_at: now_seconds(),
    };
    write_install_meta(&version_dir, &meta)?;

    Ok(InstallResult {
        meta,
        path: version_dir,
        reused: false,
    })
}

pub fn installed_versions(paths: &AppPaths) -> Result<Vec<InstalledVersion>> {
    let dir = paths.versions_dir();
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::io(format!("could not read {}", dir.display()), e)),
    };

    let mut installed = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|e| Error::io(format!("could not read {}", dir.display()), e))?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if let Some(meta) = read_install_meta(&path) {
            installed.push(InstalledVersion { meta, path });
        }
    }

    installed.sort_by(|a, b| b.meta.installed_at.cmp(&a.meta.installed_at));
    Ok(installed)
}

pub fn parse_sha256sums(text: &str) -> BTreeMap<String, String> {
    let mut sums = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(hash) = parts.next() else {
            continue;
        };
        let Some(name) = parts.next() else {
            continue;
        };
        if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            continue;
        }
        let name = name.strip_prefix('*').unwrap_or(name);
        sums.insert(name.to_owned(), hash.to_ascii_lowercase());
    }
    sums
}

pub fn validate_tag(tag: &str) -> Result<()> {
    let valid = tag == "nightly"
        || (tag.starts_with("nightly-")
            && tag.len() > "nightly-".len()
            && tag
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'.'));
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidTag(tag.to_owned()))
    }
}

fn is_game_tag(tag: &str) -> bool {
    validate_tag(tag).is_ok()
}

fn read_install_meta(version_dir: &Path) -> Option<InstallMeta> {
    let text = fs::read_to_string(version_dir.join(INSTALL_META)).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_install_meta(version_dir: &Path, meta: &InstallMeta) -> Result<()> {
    let path = version_dir.join(INSTALL_META);
    let text = serde_json::to_string_pretty(meta).map_err(|e| Error::json(&path, e))?;
    fs::write(&path, text)
        .map_err(|e| Error::io(format!("could not write {}", path.display()), e))
}

fn has_horizon_binaries(version_dir: &Path) -> bool {
    version_dir.join(binary_name("horizon-client")).is_file()
        && version_dir.join(binary_name("horizon-prepare")).is_file()
        && version_dir.join("research-lock.json").is_file()
}

fn make_executable(version_dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for name in [
            "horizon-client",
            "horizon-prepare",
            "horizon-server",
            "horizon-ctl",
        ] {
            let path = version_dir.join(name);
            if path.is_file() {
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).map_err(|e| {
                    Error::io(
                        format!("could not set permissions on {}", path.display()),
                        e,
                    )
                })?;
            }
        }
    }
    Ok(())
}

fn binary_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_owned()
    }
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_list_is_parsed() {
        let text = "\
52c15bba9cc74ab4074c4d550aec6bf75e13c218a606b90ccf96bb5d181cab7c  horizon-linux-x86_64.tar.gz
B692AE851D9DB2EDFFB6AFF89901B379F70417FE0D756C57DFB5746484814AE6 *horizon-windows-x64.zip
not-a-hash  ignored.bin
";
        let sums = parse_sha256sums(text);
        assert_eq!(
            sums.get("horizon-linux-x86_64.tar.gz").unwrap(),
            "52c15bba9cc74ab4074c4d550aec6bf75e13c218a606b90ccf96bb5d181cab7c"
        );
        assert_eq!(
            sums.get("horizon-windows-x64.zip").unwrap(),
            "b692ae851d9db2edffb6aff89901b379f70417fe0d756c57dfb5746484814ae6"
        );
        assert!(sums.get("ignored.bin").is_none());
    }

    #[test]
    fn launcher_versions_are_compared() {
        assert!(version_is_newer("0.1.0", "v0.1.1"));
        assert!(version_is_newer("0.1.1", "v0.2.0"));
        assert!(!version_is_newer("0.1.1", "v0.1.1"));
        assert!(!version_is_newer("0.2.0", "v0.1.9"));
        assert!(!version_is_newer("0.1.1", "basura"));
    }

    #[test]
    fn only_game_tags_are_accepted() {
        for tag in ["nightly", "nightly-2026-11-07", "nightly-2026.11.07"] {
            validate_tag(tag).unwrap();
        }
        for tag in [
            "launcher@0.2.54",
            "nightly/evil",
            "nightly-../evil",
            "nightly-",
            "v2026.10.23",
        ] {
            assert!(validate_tag(tag).is_err(), "{tag} was accepted");
        }
    }

    #[test]
    fn platform_assets_match_release_names() {
        assert_eq!(
            Platform::LinuxX86_64.asset_name(),
            "horizon-linux-x86_64.tar.gz"
        );
        assert_eq!(
            Platform::WindowsX64.asset_name(),
            "horizon-windows-x64.zip"
        );
        assert_eq!(
            Platform::MacosArm64.asset_name(),
            "horizon-macos-arm64.zip"
        );
    }

    #[test]
    fn install_meta_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let meta = InstallMeta {
            tag: "nightly".into(),
            asset: "horizon-linux-x86_64.tar.gz".into(),
            sha256: "52c15bba9cc74ab4074c4d550aec6bf75e13c218a606b90ccf96bb5d181cab7c".into(),
            source_url: "https://example.invalid/horizon.tar.gz".into(),
            installed_at: 42,
        };
        fs::create_dir_all(dir.path()).unwrap();
        write_install_meta(dir.path(), &meta).unwrap();
        assert_eq!(read_install_meta(dir.path()).unwrap(), meta);
    }
}
