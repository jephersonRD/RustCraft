use std::fs;
use std::path::{Path, PathBuf};

use super::error::{Error, Result};

/// Every folder the launcher owns, rooted at one data directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppPaths {
    root: PathBuf,
}

impl AppPaths {
    /// Default data directory: `RustCraft` inside the platform's user data folder.
    pub fn discover() -> Result<Self> {
        let data = dirs::data_dir().ok_or(Error::NoDataDir)?;
        Ok(Self::at(data.join("RustCraft")))
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let root = if root.is_absolute() {
            root
        } else {
            std::env::current_dir()
                .map(|current| current.join(&root))
                .unwrap_or(root)
        };
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.json")
    }

    pub fn versions_dir(&self) -> PathBuf {
        self.root.join("versions")
    }

    pub fn version_dir(&self, tag: &str) -> PathBuf {
        self.versions_dir().join(tag)
    }

    pub fn minecraft_dir(&self) -> PathBuf {
        self.root.join("minecraft")
    }

    pub fn minecraft_version_dir(&self, version: &str) -> PathBuf {
        self.minecraft_dir().join("versions").join(version)
    }

    pub fn libraries_dir(&self) -> PathBuf {
        self.minecraft_dir().join("libraries")
    }

    pub fn assets_dir(&self) -> PathBuf {
        self.minecraft_dir().join("assets")
    }

    pub fn server_cache_dir(&self) -> PathBuf {
        self.root.join("cache").join("server")
    }

    pub fn downloads_cache_dir(&self) -> PathBuf {
        self.root.join("cache").join("downloads")
    }

    pub fn resources_dir(&self) -> PathBuf {
        self.root.join("resources")
    }

    pub fn resources_dir_for(&self, tag: &str) -> PathBuf {
        self.resources_dir().join(tag)
    }

    pub fn instances_dir(&self) -> PathBuf {
        self.root.join("instances")
    }

    pub fn instance_dir(&self, slug: &str) -> PathBuf {
        self.instances_dir().join(slug)
    }

    /// Create every top-level folder the launcher writes to.
    pub fn ensure_layout(&self) -> Result<()> {
        let dirs = [
            self.root.clone(),
            self.versions_dir(),
            self.minecraft_dir(),
            self.minecraft_dir().join("versions"),
            self.libraries_dir(),
            self.assets_dir().join("indexes"),
            self.assets_dir().join("objects"),
            self.server_cache_dir(),
            self.downloads_cache_dir(),
            self.resources_dir(),
            self.instances_dir(),
        ];

        for dir in dirs {
            fs::create_dir_all(&dir)
                .map_err(|e| Error::io(format!("could not create {}", dir.display()), e))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_rooted_at_one_folder() {
        let paths = AppPaths::at("/tmp/rustcraft-test");
        assert_eq!(paths.root(), Path::new("/tmp/rustcraft-test"));
        assert_eq!(
            paths.config_file(),
            Path::new("/tmp/rustcraft-test/config.json")
        );
        assert_eq!(
            paths.version_dir("nightly"),
            Path::new("/tmp/rustcraft-test/versions/nightly")
        );
        assert_eq!(
            paths.resources_dir_for("nightly"),
            Path::new("/tmp/rustcraft-test/resources/nightly")
        );
        assert_eq!(
            paths.instance_dir("default"),
            Path::new("/tmp/rustcraft-test/instances/default")
        );
    }

    #[test]
    fn ensure_layout_creates_every_folder() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::at(dir.path().join("data"));
        paths.ensure_layout().unwrap();

        for path in [
            paths.versions_dir(),
            paths.minecraft_dir().join("versions"),
            paths.libraries_dir(),
            paths.assets_dir().join("indexes"),
            paths.assets_dir().join("objects"),
            paths.server_cache_dir(),
            paths.downloads_cache_dir(),
            paths.resources_dir(),
            paths.instances_dir(),
        ] {
            assert!(path.is_dir(), "{} was not created", path.display());
        }
    }
}
