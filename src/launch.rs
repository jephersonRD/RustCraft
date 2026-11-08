use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use crate::accounts_offline::OfflineAccount;
use crate::core::error::{Error, Result};
use crate::core::paths::AppPaths;
use crate::instances::Instance;
use crate::prepare;

const CRASH_REPORT_LINE: &str = "Horizon: crash report saved to ";

#[derive(Clone, Debug)]
pub enum Event {
    Stdout(String),
    Stderr(String),
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub code: i32,
    pub crash_report: Option<PathBuf>,
}

/// Check resources, then run horizon-client for one instance.
///
/// The account is offline: no session file, no token. `--name` makes the game
/// derive the same offline UUID on every launch.
pub fn run(
    paths: &AppPaths,
    instance: &Instance,
    account: &OfflineAccount,
    width: u32,
    height: u32,
    cancel: &AtomicBool,
    mut on_event: impl FnMut(Event),
) -> Result<Outcome> {
    let check = prepare::check(paths, &instance.version_tag)?;
    if !check.ok {
        return Err(Error::ResourcesNotReady(check.details));
    }

    let binary = client_binary(paths, &instance.version_tag)?;
    let game_dir = paths.instance_dir(&instance.id);
    std::fs::create_dir_all(&game_dir).map_err(|e| {
        Error::io(format!("could not create {}", game_dir.display()), e)
    })?;

    let mut command = Command::new(&binary);
    command
        .arg("--resources")
        .arg(paths.resources_dir_for(&instance.version_tag))
        .arg("--game-dir")
        .arg(&game_dir)
        .arg("--home")
        .arg(&game_dir)
        .arg("--width")
        .arg(width.to_string())
        .arg("--height")
        .arg(height.to_string())
        .arg("--name")
        .arg(&account.name)
        .current_dir(&game_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Proxy/VPN environment variables make Horizon's services look like an
    // illegitimate client ("connection through VPN or proxy not permitted").
    // Launch the game with a clean network so direct connections win.
    for key in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "FTP_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "ftp_proxy",
        "no_proxy",
    ] {
        command.env_remove(key);
    }

    let mut child = command.spawn().map_err(|e| {
        Error::Process(format!("could not run {}: {e}", binary.display()))
    })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::Process("horizon-client has no stdout".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| Error::Process("horizon-client has no stderr".into()))?;

    let (sender, receiver) = mpsc::channel();
    let stdout_sender = sender.clone();
    let stdout_thread = std::thread::spawn(move || {
        forward_lines(stdout, |line| {
            let _ = stdout_sender.send(Event::Stdout(line));
        });
    });
    let stderr_thread = std::thread::spawn(move || {
        forward_lines(stderr, |line| {
            let _ = sender.send(Event::Stderr(line));
        });
    });

    let mut killed = false;
    let mut crash_report = None;
    loop {
        if cancel.load(Ordering::Relaxed) && !killed {
            let _ = child.kill();
            killed = true;
        }

        match receiver.recv_timeout(Duration::from_millis(200)) {
            Ok(event) => {
                if let Event::Stderr(line) = &event {
                    if let Some(path) = line.strip_prefix(CRASH_REPORT_LINE) {
                        crash_report = Some(PathBuf::from(path.trim()));
                    }
                }
                on_event(event);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let status = child
        .wait()
        .map_err(|e| Error::Process(format!("could not wait for horizon-client: {e}")))?;
    let _ = stdout_thread.join();
    let _ = stderr_thread.join();

    if killed {
        return Err(Error::Cancelled);
    }

    Ok(Outcome {
        code: status.code().unwrap_or(-1),
        crash_report,
    })
}

fn forward_lines(reader: impl std::io::Read, mut send: impl FnMut(String)) {
    for line in BufReader::new(reader).lines() {
        match line {
            Ok(line) => send(line),
            Err(error) => {
                send(format!("could not read horizon-client output: {error}"));
                break;
            }
        }
    }
}

fn client_binary(paths: &AppPaths, tag: &str) -> Result<PathBuf> {
    let name = if cfg!(windows) {
        "horizon-client.exe"
    } else {
        "horizon-client"
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
