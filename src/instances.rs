use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::core::error::{Error, Result};
use crate::core::paths::AppPaths;

const INSTANCE_FILE: &str = "instance.json";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Instance {
    pub id: String,
    pub name: String,
    pub version_tag: String,
    pub created_at: u64,
    #[serde(default)]
    pub last_played: Option<u64>,
}

impl Instance {
    pub fn create(
        paths: &AppPaths,
        name: &str,
        version_tag: &str,
    ) -> Result<Self> {
        let name = validate_name(name)?;
        let id = unique_id(paths, &slugify(&name));
        let instance = Self {
            id: id.clone(),
            name,
            version_tag: version_tag.to_owned(),
            created_at: now(),
            last_played: None,
        };

        let dir = paths.instance_dir(&id);
        for child in ["saves", "logs", "resourcepacks", "screenshots"] {
            let path = dir.join(child);
            fs::create_dir_all(&path).map_err(|e| {
                Error::io(format!("could not create {}", path.display()), e)
            })?;
        }
        instance.save(paths)?;
        Ok(instance)
    }

    pub fn save(&self, paths: &AppPaths) -> Result<()> {
        let dir = paths.instance_dir(&self.id);
        fs::create_dir_all(&dir).map_err(|e| {
            Error::io(format!("could not create {}", dir.display()), e)
        })?;
        let path = instance_file(&dir);
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| Error::json(&path, e))?;
        fs::write(&path, text)
            .map_err(|e| Error::io(format!("could not write {}", path.display()), e))
    }

    pub fn mark_played(&mut self, paths: &AppPaths) -> Result<()> {
        self.last_played = Some(now());
        self.save(paths)
    }
}

pub fn all(paths: &AppPaths) -> Result<Vec<Instance>> {
    let dir = paths.instances_dir();
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::io(format!("could not read {}", dir.display()), e)),
    };

    let mut instances = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|e| Error::io(format!("could not read {}", dir.display()), e))?;
        if !entry.path().is_dir() {
            continue;
        }
        if let Some(instance) = load(paths, &entry.file_name().to_string_lossy())? {
            instances.push(instance);
        }
    }
    instances.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(instances)
}

pub fn load(paths: &AppPaths, id: &str) -> Result<Option<Instance>> {
    let path = instance_file(&paths.instance_dir(id));
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(Error::io(
                format!("could not read {}", path.display()),
                e,
            ))
        }
    };
    let instance = serde_json::from_str(&text).map_err(|e| Error::json(&path, e))?;
    Ok(Some(instance))
}

pub fn find(paths: &AppPaths, who: &str) -> Result<Option<Instance>> {
    if let Some(instance) = load(paths, who)? {
        return Ok(Some(instance));
    }
    Ok(all(paths)?
        .into_iter()
        .find(|instance| instance.name.eq_ignore_ascii_case(who)))
}

pub fn ensure_default(paths: &AppPaths, version_tag: &str) -> Result<Instance> {
    if let Some(instance) = all(paths)?.into_iter().next() {
        return Ok(instance);
    }
    Instance::create(paths, "Default", version_tag)
}

pub fn game_dir(paths: &AppPaths, id: &str) -> PathBuf {
    paths.instance_dir(id)
}

pub fn slugify(name: &str) -> String {
    let mut slug = String::new();
    let mut dash = false;
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            dash = false;
        } else if !dash && !slug.is_empty() {
            slug.push('-');
            dash = true;
        }
    }
    let slug = slug.trim_end_matches('-');
    let slug: String = slug.chars().take(48).collect();
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "instance".to_owned()
    } else {
        slug.to_owned()
    }
}

fn unique_id(paths: &AppPaths, base: &str) -> String {
    if !paths.instance_dir(base).exists() {
        return base.to_owned();
    }
    for number in 2.. {
        let candidate = format!("{base}-{number}");
        if !paths.instance_dir(&candidate).exists() {
            return candidate;
        }
    }
    unreachable!()
}

fn validate_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 64 {
        return Err(Error::InvalidInstanceName(format!(
            "instance name must be 1-64 characters: {name:?}"
        )));
    }
    if name.chars().any(char::is_control) || name == "." || name == ".." {
        return Err(Error::InvalidInstanceName(format!(
            "invalid instance name: {name:?}"
        )));
    }
    Ok(name.to_owned())
}

fn instance_file(dir: &Path) -> PathBuf {
    dir.join(INSTANCE_FILE)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_is_portable_and_unique_when_needed() {
        assert_eq!(slugify("Default"), "default");
        assert_eq!(slugify("Mi Mundo 2"), "mi-mundo-2");
        assert_eq!(slugify("  "), "instance");
        assert_eq!(slugify("../evil"), "evil");
        assert!(slugify("ñandú").chars().all(|c| c.is_ascii()));
    }

    #[test]
    fn instances_get_their_own_folders() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::at(dir.path());
        paths.ensure_layout().unwrap();

        let first = Instance::create(&paths, "Default", "nightly").unwrap();
        let second = Instance::create(&paths, "Default", "nightly").unwrap();
        assert_eq!(first.id, "default");
        assert_eq!(second.id, "default-2");
        assert!(paths.instance_dir("default").join("saves").is_dir());
        assert!(paths.instance_dir("default-2").join("saves").is_dir());

        let loaded = load(&paths, "default").unwrap().unwrap();
        assert_eq!(loaded.name, "Default");
        assert_eq!(all(&paths).unwrap().len(), 2);
        assert_eq!(find(&paths, "default-2").unwrap().unwrap().name, "Default");
    }

    #[test]
    fn names_are_validated() {
        assert!(validate_name("Mi mundo").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name("..").is_err());
        assert!(validate_name("bad\nname").is_err());
    }
}
