use std::path::{Path, PathBuf};
use std::process::Command;

use crate::core::paths::AppPaths;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JavaRuntime {
    pub home: PathBuf,
    pub java: PathBuf,
    pub major: u32,
    pub version: String,
}

/// Find a Java whose major version is at least `required`.
pub fn detect(
    required: u32,
    explicit_home: Option<&Path>,
    paths: &AppPaths,
) -> Option<JavaRuntime> {
    find(required, explicit_home, paths)
}

/// Find any Java, even one too old for Horizon.
pub fn detect_any(explicit_home: Option<&Path>, paths: &AppPaths) -> Option<JavaRuntime> {
    find(0, explicit_home, paths)
}

fn find(
    required: u32,
    explicit_home: Option<&Path>,
    paths: &AppPaths,
) -> Option<JavaRuntime> {
    for home in candidate_homes(explicit_home, paths) {
        let java = binary_in(&home);
        let Some(runtime) = read_version(&java, &home) else {
            continue;
        };
        if runtime.major >= required {
            return Some(runtime);
        }
    }
    None
}

fn candidate_homes(explicit_home: Option<&Path>, paths: &AppPaths) -> Vec<PathBuf> {
    let mut homes = Vec::new();
    if let Some(home) = explicit_home {
        homes.push(home.to_path_buf());
    }

    let managed = paths.root().join("java");
    homes.push(managed.join("25"));
    if let Ok(entries) = std::fs::read_dir(&managed) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                homes.push(entry.path());
                if let Ok(children) = std::fs::read_dir(entry.path()) {
                    for child in children.flatten() {
                        if child.path().is_dir() {
                            homes.push(child.path());
                        }
                    }
                }
            }
        }
    }

    if let Some(home) = std::env::var_os("JAVA_HOME") {
        homes.push(PathBuf::from(home));
    }

    // A bare `java` on PATH: the home is only used for new processes.
    homes.push(PathBuf::new());
    homes
}

fn binary_in(home: &Path) -> PathBuf {
    if home.as_os_str().is_empty() {
        return PathBuf::from(java_binary());
    }
    if home.is_file() {
        return home.to_path_buf();
    }
    home.join("bin").join(java_binary())
}

fn java_binary() -> &'static str {
    if cfg!(windows) {
        "java.exe"
    } else {
        "java"
    }
}

fn read_version(java: &Path, home: &Path) -> Option<JavaRuntime> {
    let output = Command::new(java).arg("-version").output().ok()?;
    if !output.status.success() {
        return None;
    }

    let text = if output.stderr.is_empty() {
        String::from_utf8_lossy(&output.stdout)
    } else {
        String::from_utf8_lossy(&output.stderr)
    };
    let version = text.lines().next()?.trim().to_owned();
    let major = parse_major(&version)?;
    let home = if home.as_os_str().is_empty() {
        java.parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_default()
    } else {
        home.to_path_buf()
    };

    Some(JavaRuntime {
        home,
        java: java.to_path_buf(),
        major,
        version,
    })
}

pub fn parse_major(version: &str) -> Option<u32> {
    let start = version.find('"')? + 1;
    let rest = &version[start..];
    let end = rest.find('"')?;
    let number = &rest[..end];
    number.split('.').next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_major_reads_the_quoted_version() {
        assert_eq!(
            parse_major(r#"openjdk version "25.0.1" 2026-10-20"#),
            Some(25)
        );
        assert_eq!(parse_major(r#"openjdk version "1.8.0_402""#), Some(1));
        assert_eq!(parse_major("no version here"), None);
    }

    #[test]
    fn binary_is_under_bin() {
        assert_eq!(
            binary_in(Path::new("/opt/java")),
            Path::new("/opt/java/bin").join(java_binary())
        );
        assert_eq!(binary_in(Path::new("")), PathBuf::from(java_binary()));
    }
}
