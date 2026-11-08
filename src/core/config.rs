use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::error::{Error, Result};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Config {
    pub player_name: Option<String>,
    pub selected_version: Option<String>,
    pub selected_instance: Option<String>,
    pub minecraft_dir: Option<PathBuf>,
    pub java_home: Option<PathBuf>,
    pub window: WindowConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            player_name: None,
            selected_version: None,
            selected_instance: None,
            minecraft_dir: None,
            java_home: None,
            window: WindowConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct WindowConfig {
    pub width: u32,
    pub height: u32,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
        }
    }
}

impl Config {
    /// Load a config, or return defaults when the file does not exist yet.
    pub fn load(path: &Path) -> Result<Self> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => {
                return Err(Error::io(
                    format!("could not read {}", path.display()),
                    e,
                ))
            }
        };

        serde_json::from_str(&text).map_err(|e| Error::json(path, e))
    }

    /// Write the config through a temporary file, then replace the old one.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                Error::io(format!("could not create {}", parent.display()), e)
            })?;
        }

        let text = serde_json::to_string_pretty(self)
            .map_err(|e| Error::json(path, e))?;
        let temp = temporary_path(path);

        {
            let mut file = fs::File::create(&temp)
                .map_err(|e| Error::io(format!("could not create {}", temp.display()), e))?;
            file.write_all(text.as_bytes())
                .map_err(|e| Error::io(format!("could not write {}", temp.display()), e))?;
            file.write_all(b"\n")
                .map_err(|e| Error::io(format!("could not write {}", temp.display()), e))?;
            file.sync_all()
                .map_err(|e| Error::io(format!("could not flush {}", temp.display()), e))?;
        }

        // Windows refuses a rename over an existing file.
        if path.exists() {
            fs::remove_file(path)
                .map_err(|e| Error::io(format!("could not replace {}", path.display()), e))?;
        }
        fs::rename(&temp, path)
            .map_err(|e| Error::io(format!("could not rename {}", temp.display()), e))?;
        Ok(())
    }
}

fn temporary_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.json");
    path.with_file_name(format!(".{name}.tmp"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_config_is_default() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::load(&dir.path().join("config.json")).unwrap();
        assert_eq!(config, Config::default());
        assert_eq!(config.window.width, 1280);
        assert_eq!(config.window.height, 720);
    }

    #[test]
    fn config_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let config = Config {
            player_name: Some("Alex".into()),
            selected_version: Some("nightly".into()),
            selected_instance: Some("default".into()),
            window: WindowConfig {
                width: 1600,
                height: 900,
            },
            ..Config::default()
        };

        config.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), config);
    }

    #[test]
    fn unknown_and_missing_fields_are_tolerated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, r#"{"player_name":"Steve","future_field":true}"#).unwrap();

        let config = Config::load(&path).unwrap();
        assert_eq!(config.player_name.as_deref(), Some("Steve"));
        assert_eq!(config.window, WindowConfig::default());
    }

    #[test]
    fn broken_json_is_reported_with_its_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, "{not json").unwrap();

        let error = Config::load(&path).unwrap_err();
        assert!(matches!(error, Error::Json { .. }));
    }
}
