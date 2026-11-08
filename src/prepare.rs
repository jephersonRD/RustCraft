use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use serde::Deserialize;

use crate::core::error::{Error, Result};
use crate::core::paths::AppPaths;
use crate::github_releases;
use crate::java;

pub const SUPPORTED_API: &str = "0.1.";

#[derive(Clone, Debug)]
pub enum Progress {
    Message(String),
    Fraction(f32),
    Log(String),
}

#[derive(Clone, Debug, Deserialize)]
pub struct PrepareApi {
    pub launch_api: String,
    pub horizon: String,
    pub minecraft: String,
    pub java_major: u32,
    #[serde(default)]
    pub options: Vec<ApiOption>,
    #[serde(default)]
    pub exit_codes: Vec<i32>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ApiOption {
    pub name: String,
    #[serde(default)]
    pub value: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Prepared {
    pub output: PathBuf,
    pub minecraft_version: String,
    pub java_major: u32,
    pub horizon: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckReport {
    pub ok: bool,
    pub details: String,
}

#[derive(Debug, Deserialize)]
struct ProgressLine {
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    fraction: Option<f64>,
    #[serde(default)]
    error: Option<String>,
}

enum StreamEvent {
    Stdout(String),
    Stderr(String),
}

pub fn launch_api(paths: &AppPaths, tag: &str) -> Result<PrepareApi> {
    github_releases::validate_tag(tag)?;
    let binary = preparer_binary(paths, tag)?;
    let version_dir = paths.version_dir(tag);
    let output = Command::new(&binary)
        .arg("--launch-api")
        .current_dir(&version_dir)
        .output()
        .map_err(|e| {
            Error::Process(format!("could not run {}: {e}", binary.display()))
        })?;

    if !output.status.success() {
        let details = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(Error::PrepareFailed {
            code: output.status.code().unwrap_or(-1),
            details,
        });
    }

    let api: PrepareApi = serde_json::from_slice(&output.stdout).map_err(|source| {
        Error::JsonBody {
            url: binary.display().to_string(),
            source,
        }
    })?;
    if !api.launch_api.starts_with(SUPPORTED_API) {
        return Err(Error::UnsupportedLaunchApi(api.launch_api));
    }
    Ok(api)
}

pub fn check(paths: &AppPaths, tag: &str) -> Result<CheckReport> {
    github_releases::validate_tag(tag)?;
    let binary = preparer_binary(paths, tag)?;
    let output = Command::new(&binary)
        .arg("--check")
        .arg("--output")
        .arg(paths.resources_dir_for(tag))
        .current_dir(paths.version_dir(tag))
        .output()
        .map_err(|e| {
            Error::Process(format!("could not run {}: {e}", binary.display()))
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let details = [stdout.trim(), stderr.trim()]
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n");

    match output.status.code() {
        Some(0) => Ok(CheckReport { ok: true, details }),
        Some(1) => Ok(CheckReport { ok: false, details }),
        code => Err(Error::PrepareFailed {
            code: code.unwrap_or(-1),
            details,
        }),
    }
}

pub fn prepare(
    paths: &AppPaths,
    tag: &str,
    java_home: Option<&Path>,
    cancel: &AtomicBool,
    mut progress: impl FnMut(Progress),
) -> Result<Prepared> {
    github_releases::validate_tag(tag)?;
    let api = launch_api(paths, tag)?;
    if !minecraft_ready(paths, &api.minecraft) {
        return Err(Error::MissingMinecraft(api.minecraft));
    }

    let runtime = java::detect(api.java_major, java_home, paths)
        .ok_or(Error::JavaNotFound(api.java_major))?;
    progress(Progress::Message(format!(
        "java: {} ({})",
        runtime.java.display(),
        runtime.version
    )));

    let binary = preparer_binary(paths, tag)?;
    let output = paths.resources_dir_for(tag);
    let server_jar = paths
        .server_cache_dir()
        .join(format!("{}-server.jar", api.minecraft));

    if !server_jar.is_file() {
        return Err(Error::MissingMinecraft(api.minecraft));
    }

    let mut command = Command::new(&binary);
    command
        .arg("--prepare")
        .arg("--server-jar")
        .arg(&server_jar)
        .arg("--minecraft-dir")
        .arg(paths.minecraft_dir())
        .arg("--output")
        .arg(&output)
        .arg("--complete")
        .arg("--progress")
        .arg("json")
        .current_dir(paths.version_dir(tag))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if !runtime.home.as_os_str().is_empty() {
        command.arg("--java-home").arg(&runtime.home);
    }

    progress(Progress::Message(format!(
        "preparing Horizon {} from Minecraft {}",
        api.horizon, api.minecraft
    )));
    let mut child = command.spawn().map_err(|e| {
        Error::Process(format!("could not run {}: {e}", binary.display()))
    })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::Process("horizon-prepare has no stdout".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| Error::Process("horizon-prepare has no stderr".into()))?;

    let (sender, receiver) = mpsc::channel();
    let stdout_sender = sender.clone();
    let stdout_thread = std::thread::spawn(move || {
        read_lines(stdout, |line| {
            let _ = stdout_sender.send(StreamEvent::Stdout(line));
        });
    });
    let stderr_thread = std::thread::spawn(move || {
        read_lines(stderr, |line| {
            let _ = sender.send(StreamEvent::Stderr(line));
        });
    });

    let mut killed = false;
    let mut last_error = None;
    let mut last_stderr = String::new();
    loop {
        if cancel.load(Ordering::Relaxed) && !killed {
            let _ = child.kill();
            killed = true;
        }

        match receiver.recv_timeout(Duration::from_millis(200)) {
            Ok(StreamEvent::Stdout(line)) => {
                handle_stdout(&line, &mut progress, &mut last_error);
            }
            Ok(StreamEvent::Stderr(line)) => {
                if !line.trim().is_empty() {
                    last_stderr = line.clone();
                    progress(Progress::Log(line));
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let status = child.wait().map_err(|e| {
        Error::Process(format!("could not wait for horizon-prepare: {e}"))
    })?;
    let _ = stdout_thread.join();
    let _ = stderr_thread.join();

    if killed {
        return Err(Error::Cancelled);
    }

    if !status.success() {
        let details = last_error
            .or_else(|| (!last_stderr.is_empty()).then_some(last_stderr))
            .unwrap_or_else(|| format!("exit code {}", status.code().unwrap_or(-1)));
        return Err(Error::PrepareFailed {
            code: status.code().unwrap_or(-1),
            details,
        });
    }

    progress(Progress::Message("resources ready".into()));
    Ok(Prepared {
        output,
        minecraft_version: api.minecraft,
        java_major: api.java_major,
        horizon: api.horizon,
    })
}

fn handle_stdout(
    line: &str,
    progress: &mut impl FnMut(Progress),
    last_error: &mut Option<String>,
) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }

    match serde_json::from_str::<ProgressLine>(trimmed) {
        Ok(item) => {
            if let Some(error) = item.error {
                *last_error = Some(error.clone());
                progress(Progress::Message(format!("error: {error}")));
            }
            if let Some(message) = item.message {
                progress(Progress::Message(message));
            }
            if let Some(fraction) = item.fraction {
                progress(Progress::Fraction(fraction.clamp(0.0, 1.0) as f32));
            }
        }
        Err(_) => progress(Progress::Message(trimmed.to_owned())),
    }
}

fn read_lines(reader: impl std::io::Read, mut send: impl FnMut(String)) {
    for line in BufReader::new(reader).lines() {
        match line {
            Ok(line) => send(line),
            Err(error) => {
                send(format!("could not read horizon-prepare output: {error}"));
                break;
            }
        }
    }
}

/// Whether the version JSON, client, libraries, asset index, objects, and
/// server jar for `version` are in place. It does not re-hash every asset.
pub fn minecraft_ready(paths: &AppPaths, version: &str) -> bool {
    let version_dir = paths.minecraft_version_dir(version);
    let version_json = version_dir.join(format!("{version}.json"));
    let client_jar = version_dir.join(format!("{version}.jar"));
    let server_jar = paths
        .server_cache_dir()
        .join(format!("{version}-server.jar"));

    let index_ok = fs::read_to_string(&version_json)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|json| {
            json.get("assetIndex")?
                .get("id")?
                .as_str()
                .map(str::to_owned)
        })
        .is_some_and(|id| {
            paths
                .assets_dir()
                .join("indexes")
                .join(format!("{id}.json"))
                .is_file()
        });

    let assets_ok = paths.assets_dir().join("objects").is_dir();
    version_json.is_file()
        && client_jar.is_file()
        && paths.libraries_dir().is_dir()
        && assets_ok
        && index_ok
        && server_jar.is_file()
}

fn preparer_binary(paths: &AppPaths, tag: &str) -> Result<PathBuf> {
    let name = if cfg!(windows) {
        "horizon-prepare.exe"
    } else {
        "horizon-prepare"
    };
    let binary = paths.version_dir(tag).join(name);
    if binary.is_file() {
        Ok(binary)
    } else {
        Err(Error::Install(format!(
            "{} is not installed; run `rustcraft install {tag}`",
            tag
        )))
    }
}
