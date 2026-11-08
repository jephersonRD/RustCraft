use std::io;
use std::path::PathBuf;

/// Errors shared by the launcher's core modules.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the platform has no user data directory")]
    NoDataDir,

    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: io::Error,
    },

    #[error("could not read JSON from {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("invalid JSON from {url}: {source}")]
    JsonBody {
        url: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("could not create the HTTP client: {0}")]
    HttpClient(#[source] reqwest::Error),

    #[error("HTTP request to {url} failed: {source}")]
    Http {
        url: String,
        #[source]
        source: reqwest::Error,
    },

    #[error("HTTP {status} from {url}")]
    HttpStatus { url: String, status: u16 },

    #[error("checksum mismatch for {path}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },

    #[error("download cancelled")]
    Cancelled,

    #[error("unsafe path in archive: {0}")]
    UnsafeArchivePath(String),

    #[error("archive could not be read: {0}")]
    Archive(String),

    #[error("invalid player name: {0}")]
    InvalidName(String),

    #[error("invalid instance name: {0}")]
    InvalidInstanceName(String),

    #[error("unsupported platform: {0}")]
    UnsupportedPlatform(String),

    #[error("release has no asset for this platform: {0}")]
    MissingAsset(String),

    #[error("SHA256SUMS has no entry for {0}")]
    MissingChecksum(String),

    #[error("invalid version tag: {0}")]
    InvalidTag(String),

    #[error("could not install the release: {0}")]
    Install(String),

    #[error("Minecraft version not found in the Mojang manifest: {0}")]
    MinecraftVersionNotFound(String),

    #[error("invalid Mojang metadata: {0}")]
    MojangData(String),

    #[error("Java {0} or newer was not found (set java_home or install a JRE)")]
    JavaNotFound(u32),

    #[error("Minecraft files are missing for {0}; run `rustcraft minecraft {0}` first")]
    MissingMinecraft(String),

    #[error("horizon-prepare failed with exit code {code}: {details}")]
    PrepareFailed { code: i32, details: String },

    #[error("unsupported Horizon launch API: {0}")]
    UnsupportedLaunchApi(String),

    #[error("could not run a child process: {0}")]
    Process(String),

    #[error("Horizon resources are not ready: {0}")]
    ResourcesNotReady(String),
}

impl Error {
    pub fn io(context: impl Into<String>, source: io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }

    pub fn json(path: impl Into<PathBuf>, source: serde_json::Error) -> Self {
        Self::Json {
            path: path.into(),
            source,
        }
    }

    pub fn http(url: impl Into<String>, source: reqwest::Error) -> Self {
        Self::Http {
            url: url.into(),
            source,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
