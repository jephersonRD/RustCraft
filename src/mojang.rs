use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

use reqwest::blocking::Client;
use serde::Deserialize;

use crate::core::archive;
use crate::core::download;
use crate::core::error::{Error, Result};
use crate::core::hash::Checksum;
use crate::core::paths::AppPaths;

pub const VERSION_MANIFEST_URL: &str =
    "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
pub const PINNED_VERSION: &str = "26.3";

#[derive(Clone, Debug)]
pub enum Progress {
    Message(String),
    Download {
        label: &'static str,
        downloaded: u64,
        total: Option<u64>,
    },
    Files {
        label: &'static str,
        done: u64,
        total: u64,
    },
}

#[derive(Clone, Debug)]
pub struct MinecraftFiles {
    pub version_id: String,
    pub version_json: PathBuf,
    pub client_jar: PathBuf,
    pub libraries_dir: PathBuf,
    pub assets_dir: PathBuf,
    pub server_jar: PathBuf,
    pub asset_index_id: String,
    pub java_major: Option<u32>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct VersionManifest {
    pub latest: LatestVersions,
    pub versions: Vec<VersionEntry>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LatestVersions {
    pub release: String,
    pub snapshot: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct VersionEntry {
    pub id: String,
    pub url: String,
    #[serde(default)]
    pub sha1: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VersionDetails {
    id: String,
    #[serde(default)]
    java_version: Option<JavaVersion>,
    asset_index: AssetIndexInfo,
    downloads: VersionDownloads,
    #[serde(default)]
    libraries: Vec<Library>,
}

#[derive(Clone, Debug, Deserialize)]
struct JavaVersion {
    #[serde(default)]
    major_version: Option<u32>,
}

#[derive(Clone, Debug, Deserialize)]
struct AssetIndexInfo {
    id: String,
    sha1: String,
    url: String,
}

#[derive(Clone, Debug, Deserialize)]
struct VersionDownloads {
    client: Option<DownloadInfo>,
    server: Option<DownloadInfo>,
}

#[derive(Clone, Debug, Deserialize)]
struct DownloadInfo {
    #[serde(default)]
    path: Option<String>,
    sha1: String,
    #[serde(default)]
    size: u64,
    url: String,
}

#[derive(Clone, Debug, Deserialize)]
struct Library {
    #[serde(default)]
    rules: Vec<Rule>,
    downloads: Option<LibraryDownloads>,
}

#[derive(Clone, Debug, Deserialize)]
struct LibraryDownloads {
    artifact: Option<DownloadInfo>,
}

#[derive(Clone, Debug, Deserialize)]
struct Rule {
    action: String,
    #[serde(default)]
    os: Option<OsRule>,
    #[serde(default)]
    features: Option<BTreeMap<String, bool>>,
}

#[derive(Clone, Debug, Deserialize)]
struct OsRule {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arch: Option<String>,
    #[serde(default)]
    version: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct AssetIndex {
    objects: BTreeMap<String, AssetObject>,
}

#[derive(Clone, Debug, Deserialize)]
struct AssetObject {
    hash: String,
    #[serde(default)]
    size: u64,
}

struct Job {
    url: String,
    dest: PathBuf,
    checksum: Checksum,
    size: u64,
}

/// Download the version JSON, client, libraries, assets, and server jar.
pub fn install(
    client: &Client,
    paths: &AppPaths,
    version_id: &str,
    cancel: &AtomicBool,
    mut progress: impl FnMut(Progress) + Send,
) -> Result<MinecraftFiles> {
    let version_dir = paths.minecraft_version_dir(version_id);
    fs::create_dir_all(&version_dir).map_err(|e| {
        Error::io(format!("could not create {}", version_dir.display()), e)
    })?;

    progress(Progress::Message(
        "fetching Mojang version manifest".to_owned(),
    ));
    let manifest: VersionManifest =
        download::fetch_json(client, VERSION_MANIFEST_URL)?;
    let entry = manifest
        .versions
        .iter()
        .find(|entry| entry.id == version_id)
        .ok_or_else(|| Error::MinecraftVersionNotFound(version_id.to_owned()))?;

    progress(Progress::Message(format!(
        "reading version JSON for {version_id}"
    )));
    let raw_version = download::fetch_text(client, &entry.url)?;
    let details: VersionDetails = serde_json::from_str(&raw_version).map_err(|source| {
        Error::JsonBody {
            url: entry.url.clone(),
            source,
        }
    })?;
    if details.id != version_id {
        return Err(Error::MojangData(format!(
            "manifest asked for {version_id}, version JSON says {}",
            details.id
        )));
    }

    let version_json = version_dir.join(format!("{version_id}.json"));
    fs::write(&version_json, &raw_version).map_err(|e| {
        Error::io(format!("could not write {}", version_json.display()), e)
    })?;

    let client_jar = version_dir.join(format!("{version_id}.jar"));
    let client_info = details
        .downloads
        .client
        .as_ref()
        .ok_or_else(|| Error::MojangData("version JSON has no client jar".into()))?;
    download_one(client, client_info, &client_jar, cancel, &mut progress, "client")?;

    let server_jar = paths
        .server_cache_dir()
        .join(format!("{version_id}-server.jar"));
    let server_info = details
        .downloads
        .server
        .as_ref()
        .ok_or_else(|| Error::MojangData("version JSON has no server jar".into()))?;
    download_one(client, server_info, &server_jar, cancel, &mut progress, "server")?;

    let library_jobs = library_jobs(&details.libraries, &paths.libraries_dir())?;
    progress(Progress::Message(format!(
        "downloading {} libraries",
        library_jobs.len()
    )));
    download_many(
        client,
        library_jobs,
        cancel,
        "libraries",
        &mut progress,
    )?;

    let index_path = paths
        .assets_dir()
        .join("indexes")
        .join(format!("{}.json", details.asset_index.id));
    let index_checksum = Checksum::Sha1(details.asset_index.sha1.clone());
    progress(Progress::Message(format!(
        "downloading asset index {}",
        details.asset_index.id
    )));
    download::download(
        client,
        &details.asset_index.url,
        &index_path,
        Some(&index_checksum),
        cancel,
        |_, _| {},
    )?;

    let index_text = fs::read_to_string(&index_path).map_err(|e| {
        Error::io(format!("could not read {}", index_path.display()), e)
    })?;
    let asset_index: AssetIndex = serde_json::from_str(&index_text)
        .map_err(|e| Error::json(&index_path, e))?;
    let asset_jobs = asset_jobs(&asset_index, &paths.assets_dir())?;
    progress(Progress::Message(format!(
        "downloading {} assets",
        asset_jobs.len()
    )));
    download_many(client, asset_jobs, cancel, "assets", &mut progress)?;

    progress(Progress::Message("Minecraft files ready".to_owned()));
    Ok(MinecraftFiles {
        version_id: version_id.to_owned(),
        version_json,
        client_jar,
        libraries_dir: paths.libraries_dir(),
        assets_dir: paths.assets_dir(),
        server_jar,
        asset_index_id: details.asset_index.id,
        java_major: details.java_version.and_then(|java| java.major_version),
    })
}

fn download_one(
    client: &Client,
    info: &DownloadInfo,
    dest: &Path,
    cancel: &AtomicBool,
    progress: &mut (impl FnMut(Progress) + Send),
    label: &'static str,
) -> Result<()> {
    progress(Progress::Message(format!(
        "downloading {label} ({:.1} MB)",
        info.size as f64 / 1_000_000.0
    )));
    let checksum = Checksum::Sha1(info.sha1.clone());
    download::download(
        client,
        &info.url,
        dest,
        Some(&checksum),
        cancel,
        |downloaded, total| {
            progress(Progress::Download {
                label,
                downloaded,
                total,
            });
        },
    )
}

fn library_jobs(libraries: &[Library], libraries_dir: &Path) -> Result<Vec<Job>> {
    let mut jobs = Vec::new();
    for library in libraries {
        if !rules_allow(&library.rules) {
            continue;
        }
        let Some(artifact) = library
            .downloads
            .as_ref()
            .and_then(|downloads| downloads.artifact.as_ref())
        else {
            continue;
        };
        let path = artifact
            .path
            .as_deref()
            .ok_or_else(|| Error::MojangData("library artifact has no path".into()))?;
        let relative = archive::safe_relative_path(Path::new(path))?;
        jobs.push(Job {
            url: artifact.url.clone(),
            dest: libraries_dir.join(relative),
            checksum: Checksum::Sha1(artifact.sha1.clone()),
            size: artifact.size,
        });
    }
    Ok(jobs)
}

fn asset_jobs(index: &AssetIndex, assets_dir: &Path) -> Result<Vec<Job>> {
    let objects = assets_dir.join("objects");
    let mut jobs = Vec::with_capacity(index.objects.len());
    for object in index.objects.values() {
        if object.hash.len() != 40
            || !object.hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(Error::MojangData(format!(
                "asset has an invalid SHA-1: {}",
                object.hash
            )));
        }
        let prefix = &object.hash[..2];
        jobs.push(Job {
            url: format!(
                "https://resources.download.minecraft.net/{prefix}/{}",
                object.hash
            ),
            dest: objects.join(prefix).join(&object.hash),
            checksum: Checksum::Sha1(object.hash.clone()),
            size: object.size,
        });
    }
    Ok(jobs)
}

fn download_many(
    client: &Client,
    jobs: Vec<Job>,
    cancel: &AtomicBool,
    label: &'static str,
    progress: &mut (impl FnMut(Progress) + Send),
) -> Result<()> {
    if jobs.is_empty() {
        return Ok(());
    }

    let total: u64 = jobs.iter().map(|job| job.size).sum();
    let next = AtomicUsize::new(0);
    let done = AtomicU64::new(0);
    let failed: Mutex<Option<Error>> = Mutex::new(None);
    let progress = Mutex::new(progress);
    let workers = std::thread::available_parallelism()
        .map_or(4, |count| count.get())
        .clamp(1, 8);
    let count = jobs.len();

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                if cancel.load(Ordering::Relaxed)
                    || failed.lock().map_or(true, |error| error.is_some())
                {
                    break;
                }

                let index = next.fetch_add(1, Ordering::Relaxed);
                if index >= count {
                    break;
                }
                let job = &jobs[index];
                let result = download::download(
                    client,
                    &job.url,
                    &job.dest,
                    Some(&job.checksum),
                    cancel,
                    |_, _| {},
                );

                if let Err(error) = result {
                    if let Ok(mut slot) = failed.lock() {
                        if slot.is_none() {
                            *slot = Some(error);
                        }
                    }
                    break;
                }

                let downloaded = done.fetch_add(job.size, Ordering::Relaxed) + job.size;
                if let Ok(mut callback) = progress.lock() {
                    let callback: &mut dyn FnMut(Progress) = &mut **callback;
                    callback(Progress::Files {
                        label,
                        done: downloaded,
                        total,
                    });
                }
            });
        }
    });

    match failed.into_inner() {
        Ok(Some(error)) => Err(error),
        _ => Ok(()),
    }
}

fn rules_allow(rules: &[Rule]) -> bool {
    if rules.is_empty() {
        return true;
    }

    let mut allowed = false;
    for rule in rules {
        if !rule_matches(rule) {
            continue;
        }
        match rule.action.as_str() {
            "allow" => allowed = true,
            "disallow" => allowed = false,
            _ => {}
        }
    }
    allowed
}

fn rule_matches(rule: &Rule) -> bool {
    if let Some(os) = &rule.os {
        if let Some(name) = &os.name {
            if name != os_name() {
                return false;
            }
        }
        if let Some(arch) = &os.arch {
            if arch != os_arch() {
                return false;
            }
        }
        // Version constraints are rare and not used by 26.3.
        let _ = &os.version;
    }

    if let Some(features) = &rule.features {
        for (feature, needed) in features {
            let enabled = feature == "is_demo_user" && false;
            if enabled != *needed {
                return false;
            }
        }
    }

    true
}

fn os_name() -> &'static str {
    match std::env::consts::OS {
        "macos" => "osx",
        other => other,
    }
}

fn os_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_follow_os_and_order() {
        let linux_allow: Rule = serde_json::from_str(
            r#"{"action":"allow","os":{"name":"linux"}}"#,
        )
        .unwrap();
        let windows_allow: Rule = serde_json::from_str(
            r#"{"action":"allow","os":{"name":"windows"}}"#,
        )
        .unwrap();
        let allow_all: Rule =
            serde_json::from_str(r#"{"action":"allow"}"#).unwrap();
        let deny_all: Rule =
            serde_json::from_str(r#"{"action":"disallow"}"#).unwrap();

        assert!(rules_allow(&[]));
        assert_eq!(
            rules_allow(&[linux_allow.clone()]),
            cfg!(target_os = "linux")
        );
        assert!(!rules_allow(&[windows_allow]));
        assert!(rules_allow(&[allow_all.clone()]));
        assert!(!rules_allow(&[allow_all, deny_all]));
    }

    #[test]
    fn features_are_disabled() {
        let rule: Rule = serde_json::from_str(
            r#"{"action":"allow","features":{"is_demo_user":true}}"#,
        )
        .unwrap();
        assert!(!rule_matches(&rule));

        let rule: Rule = serde_json::from_str(
            r#"{"action":"allow","features":{"is_demo_user":false}}"#,
        )
        .unwrap();
        assert!(rule_matches(&rule));
    }

    #[test]
    fn asset_paths_use_two_character_prefixes() {
        let index: AssetIndex = serde_json::from_str(
            r#"{"objects":{"minecraft/sounds/a.ogg":{"hash":"0123456789abcdef0123456789abcdef01234567","size":12}}}"#,
        )
        .unwrap();
        let jobs = asset_jobs(&index, Path::new("/assets")).unwrap();
        assert_eq!(jobs.len(), 1);
        assert!(jobs[0]
            .dest
            .ends_with("objects/01/0123456789abcdef0123456789abcdef01234567"));
        assert_eq!(jobs[0].size, 12);
    }

    #[test]
    fn bad_asset_hashes_are_rejected() {
        let index: AssetIndex = serde_json::from_str(
            r#"{"objects":{"x":{"hash":"../../etc/passwd","size":1}}}"#,
        )
        .unwrap();
        assert!(asset_jobs(&index, Path::new("/assets")).is_err());
    }
}
