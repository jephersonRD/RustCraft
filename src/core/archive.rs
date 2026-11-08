use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};

use flate2::read::GzDecoder;

use super::error::{Error, Result};

/// Extract a `.zip`, `.tar.gz`, or `.tgz` archive under `dest`.
pub fn extract(archive: &Path, dest: &Path) -> Result<()> {
    let name = archive
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    if name.ends_with(".zip") {
        extract_zip(archive, dest)
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        extract_tar_gz(archive, dest)
    } else {
        Err(Error::Archive(format!(
            "unsupported archive format: {}",
            archive.display()
        )))
    }
}

pub fn extract_zip(archive: &Path, dest: &Path) -> Result<()> {
    let file = File::open(archive)
        .map_err(|e| Error::io(format!("could not open {}", archive.display()), e))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| Error::Archive(format!("{}: {e}", archive.display())))?;

    fs::create_dir_all(dest)
        .map_err(|e| Error::io(format!("could not create {}", dest.display()), e))?;

    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|e| Error::Archive(format!("{}: {e}", archive.display())))?;

        let entry_name = entry
            .name()
            .map_err(|e| Error::Archive(format!("{}: {e}", archive.display())))?
            .into_owned();
        let raw = entry
            .enclosed_name()
            .ok_or_else(|| Error::UnsafeArchivePath(entry_name.clone()))?;
        let relative = safe_relative_path(&raw)?;
        let target = dest.join(&relative);
        let is_dir = entry.is_dir() || entry_name.ends_with('/');

        if is_symlink(entry.unix_mode()) {
            return Err(Error::UnsafeArchivePath(format!(
                "{} is a symlink",
                relative.display()
            )));
        }

        if is_dir {
            fs::create_dir_all(&target)
                .map_err(|e| Error::io(format!("could not create {}", target.display()), e))?;
            continue;
        }

        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                Error::io(format!("could not create {}", parent.display()), e)
            })?;
        }

        let mut output = File::create(&target)
            .map_err(|e| Error::io(format!("could not create {}", target.display()), e))?;
        io::copy(&mut entry, &mut output)
            .map_err(|e| Error::io(format!("could not extract {}", target.display()), e))?;

        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let mode = mode & 0o777;
            if mode != 0 {
                fs::set_permissions(&target, fs::Permissions::from_mode(mode)).map_err(|e| {
                    Error::io(
                        format!("could not set permissions on {}", target.display()),
                        e,
                    )
                })?;
            }
        }
    }

    Ok(())
}

pub fn extract_tar_gz(archive: &Path, dest: &Path) -> Result<()> {
    let file = File::open(archive)
        .map_err(|e| Error::io(format!("could not open {}", archive.display()), e))?;
    let decoder = GzDecoder::new(file);
    let mut tar = tar::Archive::new(decoder);

    fs::create_dir_all(dest)
        .map_err(|e| Error::io(format!("could not create {}", dest.display()), e))?;

    let entries = tar
        .entries()
        .map_err(|e| Error::Archive(format!("{}: {e}", archive.display())))?;

    for entry in entries {
        let mut entry =
            entry.map_err(|e| Error::Archive(format!("{}: {e}", archive.display())))?;
        let raw = entry
            .path()
            .map_err(|e| Error::Archive(format!("{}: {e}", archive.display())))?
            .into_owned();
        let relative = safe_relative_path(&raw)?;
        let kind = entry.header().entry_type();

        if !kind.is_file() && !kind.is_dir() {
            return Err(Error::UnsafeArchivePath(format!(
                "{} is not a regular file or directory",
                relative.display()
            )));
        }

        let unpacked = entry.unpack_in(dest).map_err(|e| {
            Error::io(
                format!("could not extract {}", relative.display()),
                e,
            )
        })?;
        if !unpacked {
            return Err(Error::UnsafeArchivePath(relative.display().to_string()));
        }
    }

    Ok(())
}

/// Extract an archive whose contents share one root folder, moving that root
/// directly to `dest`. Used for release archives such as
/// `horizon-linux-x86_64/`.
pub fn extract_flat(archive: &Path, dest: &Path) -> Result<()> {
    let parent = dest
        .parent()
        .ok_or_else(|| Error::Archive(format!("{} has no parent folder", dest.display())))?;
    fs::create_dir_all(parent)
        .map_err(|e| Error::io(format!("could not create {}", parent.display()), e))?;

    let temp = unique_temp_dir(parent, dest);
    fs::create_dir_all(&temp)
        .map_err(|e| Error::io(format!("could not create {}", temp.display()), e))?;

    let result = extract(archive, &temp).and_then(|()| move_single_root(&temp, dest));
    if temp.exists() {
        let _ = fs::remove_dir_all(&temp);
    }
    result
}

fn move_single_root(temp: &Path, dest: &Path) -> Result<()> {
    let entries = fs::read_dir(temp)
        .map_err(|e| Error::io(format!("could not read {}", temp.display()), e))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| Error::io(format!("could not read {}", temp.display()), e))?;

    let source = if entries.len() == 1 && entries[0].path().is_dir() {
        entries[0].path()
    } else {
        return Err(Error::Archive(
            "the archive does not contain exactly one root folder".to_owned(),
        ));
    };

    if dest.exists() {
        if dest.is_dir() {
            fs::remove_dir_all(dest)
                .map_err(|e| Error::io(format!("could not replace {}", dest.display()), e))?;
        } else {
            fs::remove_file(dest)
                .map_err(|e| Error::io(format!("could not replace {}", dest.display()), e))?;
        }
    }

    fs::rename(&source, dest).map_err(|e| {
        Error::io(
            format!("could not move {} into {}", source.display(), dest.display()),
            e,
        )
    })
}

fn unique_temp_dir(parent: &Path, dest: &Path) -> PathBuf {
    let name = dest
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("extract");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |time| time.as_nanos());
    parent.join(format!(
        ".{name}.extract-{}-{nanos}",
        std::process::id()
    ))
}

/// Reject absolute paths, parent traversal, and platform prefixes.
pub fn safe_relative_path(path: &Path) -> Result<PathBuf> {
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(Error::UnsafeArchivePath(path.display().to_string()));
            }
        }
    }

    if clean.as_os_str().is_empty() {
        return Err(Error::UnsafeArchivePath(path.display().to_string()));
    }
    Ok(clean)
}

fn is_symlink(mode: Option<u32>) -> bool {
    const KIND_MASK: u32 = 0o170_000;
    const SYMLINK: u32 = 0o120_000;
    mode.is_some_and(|mode| mode & KIND_MASK == SYMLINK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn safe_paths_stay_relative() {
        assert_eq!(
            safe_relative_path(Path::new("root/dir/file.txt")).unwrap(),
            PathBuf::from("root/dir/file.txt")
        );
        assert_eq!(
            safe_relative_path(Path::new("./file.txt")).unwrap(),
            PathBuf::from("file.txt")
        );

        for unsafe_path in ["../escape", "a/../../escape", "/etc/passwd", ""] {
            assert!(
                safe_relative_path(Path::new(unsafe_path)).is_err(),
                "{unsafe_path} was accepted"
            );
        }
    }

    #[test]
    fn zip_round_trip_keeps_files_and_modes() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("files.zip");
        {
            let file = File::create(&archive).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            zip.add_directory("root/", options).unwrap();
            zip.start_file("root/readme.txt", options).unwrap();
            zip.write_all(b"hello").unwrap();
            zip.finish().unwrap();
        }

        let dest = dir.path().join("out");
        extract_zip(&archive, &dest).unwrap();
        assert_eq!(
            fs::read_to_string(dest.join("root/readme.txt")).unwrap(),
            "hello"
        );
    }

    #[test]
    fn flat_extraction_strips_the_single_root() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("release.zip");
        {
            let file = File::create(&archive).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            zip.add_directory("horizon-test/", options).unwrap();
            zip.start_file("horizon-test/horizon-client", options)
                .unwrap();
            zip.write_all(b"binary").unwrap();
            zip.finish().unwrap();
        }

        let dest = dir.path().join("out");
        extract_flat(&archive, &dest).unwrap();
        assert_eq!(
            fs::read_to_string(dest.join("horizon-client")).unwrap(),
            "binary"
        );
        assert!(!dest.join("horizon-test").exists());
    }

    #[test]
    fn tar_round_trip_keeps_files() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("files.tar.gz");
        {
            let file = File::create(&archive).unwrap();
            let encoder = flate2::write::GzEncoder::new(
                file,
                flate2::Compression::default(),
            );
            let mut tar = tar::Builder::new(encoder);
            let mut header = tar::Header::new_gnu();
            header.set_size(5);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, "root/readme.txt", &b"hello"[..])
                .unwrap();
            tar.into_inner().unwrap().finish().unwrap();
        }

        let dest = dir.path().join("out");
        extract_tar_gz(&archive, &dest).unwrap();
        assert_eq!(
            fs::read_to_string(dest.join("root/readme.txt")).unwrap(),
            "hello"
        );
    }

    #[test]
    fn extraction_rejects_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("evil.tar.gz");
        {
            let file = File::create(&archive).unwrap();
            let encoder = flate2::write::GzEncoder::new(
                file,
                flate2::Compression::default(),
            );
            let mut tar = tar::Builder::new(encoder);
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            header.set_mode(0o777);
            header.set_link_name("/etc/passwd").unwrap();
            header.set_cksum();
            tar.append_data(&mut header, "root/link", std::io::empty())
                .unwrap();
            tar.into_inner().unwrap().finish().unwrap();
        }

        let dest = dir.path().join("out");
        let error = extract_tar_gz(&archive, &dest).unwrap_err();
        assert!(matches!(error, Error::UnsafeArchivePath(_)));
    }
}
