use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use reqwest::blocking::{Client, Response};
use serde::de::DeserializeOwned;

use super::error::{Error, Result};
use super::hash::Checksum;

pub const USER_AGENT: &str = concat!("RustCraft/", env!("CARGO_PKG_VERSION"));
const READ_BUFFER: usize = 64 * 1024;

/// Build the blocking HTTP client used by every network module.
pub fn client() -> Result<Client> {
    Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(std::time::Duration::from_secs(30))
        // A dead connection used to hang downloads forever. Fifteen minutes
        // is enough for the biggest file even on a slow link.
        .timeout(std::time::Duration::from_secs(15 * 60))
        .build()
        .map_err(Error::HttpClient)
}

/// Download `url` to `dest`, optionally checking a digest before the file is kept.
///
/// The bytes go to a `.part` file next to the destination. On success the part
/// file is renamed; on failure it is removed. An existing file that already
/// matches the checksum is reused without a request.
pub fn download(
    client: &Client,
    url: &str,
    dest: &Path,
    checksum: Option<&Checksum>,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<()> {
    if dest.is_file() {
        match checksum {
            Some(check) if check.verify(dest).is_ok() => return Ok(()),
            None => return Ok(()),
            Some(_) => {
                fs::remove_file(dest).map_err(|e| {
                    Error::io(format!("could not replace {}", dest.display()), e)
                })?;
            }
        }
    }

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            Error::io(format!("could not create {}", parent.display()), e)
        })?;
    }

    let part = part_path(dest);

    // A flaky connection occasionally delivers a body that does not match the
    // published checksum. Retry a couple of times before giving up.
    let mut last_error = None;
    for attempt in 1..=3 {
        match download_to_part(client, url, &part, cancel, &mut progress) {
            Ok(()) => match checksum {
                Some(check) => match check.verify(&part) {
                    Ok(()) => break,
                    Err(error) => {
                        let _ = fs::remove_file(&part);
                        last_error = Some(error);
                        if attempt < 3 {
                            progress(0, None);
                            std::thread::sleep(std::time::Duration::from_secs(1));
                        }
                    }
                },
                None => break,
            },
            Err(error) => {
                let _ = fs::remove_file(&part);
                let cancelled = matches!(error, Error::Cancelled);
                last_error = Some(error);
                if cancelled || attempt == 3 {
                    return Err(last_error.unwrap_or_else(|| {
                        Error::Process("download failed".into())
                    }));
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }
        if cancel.load(Ordering::Relaxed) {
            let _ = fs::remove_file(&part);
            return Err(Error::Cancelled);
        }
    }

    if !part.is_file() {
        return Err(last_error.unwrap_or_else(|| {
            Error::Process(format!("could not download {url}"))
        }));
    }

    if dest.exists() {
        fs::remove_file(dest)
            .map_err(|e| Error::io(format!("could not replace {}", dest.display()), e))?;
    }
    fs::rename(&part, dest).map_err(|e| {
        Error::io(
            format!("could not move {} into place", dest.display()),
            e,
        )
    })?;
    Ok(())
}

fn download_to_part(
    client: &Client,
    url: &str,
    part: &Path,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(u64, Option<u64>),
) -> Result<()> {
    let mut response = client.get(url).send().map_err(|e| Error::http(url, e))?;
    let status = response.status();
    if !status.is_success() {
        return Err(Error::HttpStatus {
            url: url.to_owned(),
            status: status.as_u16(),
        });
    }

    let total = response.content_length();
    let mut file = fs::File::create(part)
        .map_err(|e| Error::io(format!("could not create {}", part.display()), e))?;
    let mut buffer = vec![0u8; READ_BUFFER];
    let mut downloaded = 0u64;
    progress(downloaded, total);

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }

        let read = response
            .read(&mut buffer)
            .map_err(|e| Error::io(format!("could not download {url}"), e))?;
        if read == 0 {
            break;
        }

        file.write_all(&buffer[..read])
            .map_err(|e| Error::io(format!("could not write {}", part.display()), e))?;
        downloaded += read as u64;
        progress(downloaded, total);
    }

    file.flush()
        .map_err(|e| Error::io(format!("could not flush {}", part.display()), e))?;
    Ok(())
}

fn part_path(dest: &Path) -> PathBuf {
    let name = dest
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("download");
    dest.with_file_name(format!(".{name}.part"))
}

/// Fetch a small text document, with a request timeout.
pub fn fetch_text(client: &Client, url: &str) -> Result<String> {
    let response = request(client, url)?;
    response.text().map_err(|e| Error::http(url, e))
}

/// Fetch and parse a small JSON document.
pub fn fetch_json<T: DeserializeOwned>(client: &Client, url: &str) -> Result<T> {
    let response = request(client, url)?;
    let text = response.text().map_err(|e| Error::http(url, e))?;
    serde_json::from_str(&text).map_err(|source| Error::JsonBody {
        url: url.to_owned(),
        source,
    })
}

fn request(client: &Client, url: &str) -> Result<Response> {
    let response = client
        .get(url)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .map_err(|e| Error::http(url, e))?;
    let status = response.status();
    if !status.is_success() {
        return Err(Error::HttpStatus {
            url: url.to_owned(),
            status: status.as_u16(),
        });
    }
    Ok(response)
}
