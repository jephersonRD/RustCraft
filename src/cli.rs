use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use clap::{Parser, Subcommand};

use crate::accounts_offline::OfflineAccount;
use crate::core::config::Config;
use crate::core::download;
use crate::core::paths::AppPaths;
use crate::github_releases::{self, InstallEvent, Platform};
use crate::instances::{self, Instance};
use crate::java;
use crate::launch;
use crate::mojang::{self, Progress as MojangProgress};
use crate::prepare::{self, Progress as PrepareProgress};

#[derive(Debug, Parser)]
#[command(
    name = "rustcraft",
    version,
    about = "Offline launcher for Horizon, Minecraft Java Edition in Rust",
    arg_required_else_help = false
)]
pub struct Cli {
    /// Override the launcher's data directory.
    #[arg(long, global = true, env = "RUSTCRAFT_DATA_DIR", value_name = "DIR")]
    pub data_dir: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Show paths, the saved player, and whether Java is available.
    Doctor,
    /// Print every folder the launcher uses.
    Paths,
    /// Print the saved configuration.
    Config,
    /// Validate and save the offline player name.
    Name { name: String },
    /// List installed Horizon versions, or available ones with --remote.
    Versions {
        #[arg(long)]
        remote: bool,
    },
    /// Download and install a Horizon release (default: nightly).
    Install {
        #[arg(default_value = github_releases::DEFAULT_TAG)]
        tag: String,
    },
    /// Download Minecraft files from Mojang and verify every SHA-1.
    Minecraft {
        #[arg(default_value = mojang::PINNED_VERSION)]
        version: String,
    },
    /// Show or save the Java runtime used by horizon-prepare.
    Java { path: Option<PathBuf> },
    /// Download Minecraft, prepare Horizon resources, and verify them.
    Prepare {
        version: Option<String>,
        #[arg(long)]
        force: bool,
        #[arg(long, value_name = "DIR")]
        java_home: Option<PathBuf>,
    },
    /// List, create, or remove instances.
    Instance {
        #[command(subcommand)]
        action: InstanceCommand,
    },
    /// Launch Horizon for an instance with the saved offline player.
    Play {
        /// Instance id or name (default: the first one, created if needed).
        instance: Option<String>,
        #[arg(long)]
        width: Option<u32>,
        #[arg(long)]
        height: Option<u32>,
    },
    /// Open the graphical launcher (the default when no command is given).
    Gui,
}

#[derive(Debug, Subcommand)]
pub enum InstanceCommand {
    /// List every instance.
    List,
    /// Create an instance, optionally for a specific Horizon version.
    Create {
        name: String,
        #[arg(long)]
        version: Option<String>,
    },
    /// Delete an instance and its worlds.
    Remove { instance: String },
}

pub fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let paths = match cli.data_dir {
        Some(root) => AppPaths::at(root),
        None => AppPaths::discover()?,
    };

    let Some(command) = cli.command else {
        return crate::gui::run(paths).map_err(|error| anyhow::anyhow!("{error}"));
    };

    match command {
        Command::Doctor => doctor(&paths)?,
        Command::Paths => print_paths(&paths),
        Command::Config => print_config(&paths)?,
        Command::Name { name } => save_name(&paths, &name)?,
        Command::Versions { remote } => versions(&paths, remote)?,
        Command::Install { tag } => install(&paths, &tag)?,
        Command::Minecraft { version } => minecraft(&paths, &version)?,
        Command::Java { path } => java_command(&paths, path)?,
        Command::Prepare {
            version,
            force,
            java_home,
        } => prepare_command(&paths, version, force, java_home)?,
        Command::Instance { action } => instance_command(&paths, action)?,
        Command::Play {
            instance,
            width,
            height,
        } => play(&paths, instance, width, height)?,
        Command::Gui => {
            crate::gui::run(paths).map_err(|error| anyhow::anyhow!("{error}"))?;
        }
    }
    Ok(())
}

fn doctor(paths: &AppPaths) -> anyhow::Result<()> {
    paths.ensure_layout()?;
    let config = Config::load(&paths.config_file())?;

    println!("RustCraft {}", env!("CARGO_PKG_VERSION"));
    println!("data: {}", paths.root().display());
    println!("config: {}", paths.config_file().display());

    match config.player_name.as_deref() {
        Some(name) => match OfflineAccount::new(name) {
            Ok(account) => println!("player: {} ({})", account.name, account.uuid),
            Err(error) => println!("player: invalid saved name: {error}"),
        },
        None => println!("player: not set (run `rustcraft name <NAME>`)"),
    }

    let installed = github_releases::installed_versions(paths)?;
    println!("versions: {} installed", installed.len());

    match java::detect_any(config.java_home.as_deref(), paths) {
        Some(runtime) => {
            println!(
                "java: {} ({})",
                runtime.java.display(),
                runtime.version
            );
            if runtime.major < 25 {
                println!("warning: Horizon 26.3 needs Java 25");
            }
        }
        None => println!("java: not found (Horizon preparation needs Java 25)"),
    }

    Ok(())
}

fn print_paths(paths: &AppPaths) {
    for (label, path) in [
        ("root", paths.root().to_path_buf()),
        ("config", paths.config_file()),
        ("versions", paths.versions_dir()),
        ("minecraft", paths.minecraft_dir()),
        ("libraries", paths.libraries_dir()),
        ("assets", paths.assets_dir()),
        ("resources", paths.resources_dir()),
        ("instances", paths.instances_dir()),
        ("cache", paths.downloads_cache_dir()),
    ] {
        println!("{label}: {}", path.display());
    }
}

fn print_config(paths: &AppPaths) -> anyhow::Result<()> {
    let config = Config::load(&paths.config_file())?;
    println!("{}", serde_json::to_string_pretty(&config)?);
    Ok(())
}

fn save_name(paths: &AppPaths, name: &str) -> anyhow::Result<()> {
    let account = OfflineAccount::new(name)?;
    let mut config = Config::load(&paths.config_file())?;
    config.player_name = Some(account.name.clone());
    config.save(&paths.config_file())?;
    println!("player: {} ({})", account.name, account.uuid);
    Ok(())
}

fn versions(paths: &AppPaths, remote: bool) -> anyhow::Result<()> {
    if remote {
        let client = download::client()?;
        let releases = github_releases::list_releases(&client, 30)?;
        if releases.is_empty() {
            println!("no Horizon game releases found");
            return Ok(());
        }
        for release in releases {
            println!(
                "{}\t{}",
                release.tag_name,
                release.published_at.as_deref().unwrap_or("-")
            );
        }
        return Ok(());
    }

    let installed = github_releases::installed_versions(paths)?;
    if installed.is_empty() {
        println!("no versions installed (run `rustcraft install`)");
        return Ok(());
    }

    let config = Config::load(&paths.config_file())?;
    for version in installed {
        let marker = if config.selected_version.as_deref() == Some(&version.meta.tag) {
            "*"
        } else {
            " "
        };
        println!(
            "{marker} {}\t{}\t{}",
            version.meta.tag,
            version.meta.asset,
            version.path.display()
        );
    }
    Ok(())
}

fn install(paths: &AppPaths, tag: &str) -> anyhow::Result<()> {
    paths.ensure_layout()?;
    let client = download::client()?;
    let platform = Platform::current()?;

    println!("release: {tag}");
    let release = github_releases::fetch_release(&client, tag)?;
    println!("asset: {}", platform.asset_name());

    let cancel = AtomicBool::new(false);
    let mut last_step = None;
    let result = github_releases::install_release(
        &client,
        paths,
        &release,
        platform,
        &cancel,
        |event| match event {
            InstallEvent::Message(message) => println!("{message}"),
            InstallEvent::Download { downloaded, total } => {
                let Some(total) = total.filter(|total| *total > 0) else {
                    return;
                };
                let percent = ((downloaded * 100) / total) as u32;
                let step = percent / 5;
                if last_step != Some(step) {
                    last_step = Some(step);
                    println!("  {percent}% ({downloaded}/{total} bytes)");
                }
            }
        },
    )?;

    if result.reused {
        println!("already installed: {}", result.path.display());
    } else {
        println!("installed: {}", result.path.display());
    }
    println!("sha256: {}", result.meta.sha256);

    let mut config = Config::load(&paths.config_file())?;
    config.selected_version = Some(result.meta.tag.clone());
    config.save(&paths.config_file())?;
    println!("selected version: {}", result.meta.tag);
    Ok(())
}

fn minecraft(paths: &AppPaths, version: &str) -> anyhow::Result<()> {
    paths.ensure_layout()?;
    let client = download::client()?;
    let cancel = AtomicBool::new(false);
    let mut steps: HashMap<&'static str, u32> = HashMap::new();

    let files = mojang::install(&client, paths, version, &cancel, |event| {
        match event {
            MojangProgress::Message(message) => println!("{message}"),
            MojangProgress::Download {
                label,
                downloaded,
                total,
            } => {
                let Some(total) = total.filter(|total| *total > 0) else {
                    return;
                };
                print_step(&mut steps, label, downloaded, total);
            }
            MojangProgress::Files {
                label,
                done,
                total,
            } => {
                if total > 0 {
                    print_step(&mut steps, label, done, total);
                }
            }
        }
    })?;

    println!("minecraft: {}", files.version_id);
    println!("version json: {}", files.version_json.display());
    println!("client jar: {}", files.client_jar.display());
    println!("libraries: {}", files.libraries_dir.display());
    println!("assets: {}", files.assets_dir.display());
    println!("asset index: {}", files.asset_index_id);
    println!("server jar: {}", files.server_jar.display());
    if let Some(major) = files.java_major {
        println!("java major: {major}");
    }
    Ok(())
}

fn print_step(
    steps: &mut HashMap<&'static str, u32>,
    label: &'static str,
    done: u64,
    total: u64,
) {
    let percent = ((done.min(total) * 100) / total) as u32;
    let step = percent / 5;
    if steps.insert(label, step) != Some(step) {
        println!("  {label}: {percent}%");
    }
}

fn java_command(paths: &AppPaths, path: Option<PathBuf>) -> anyhow::Result<()> {
    let mut config = Config::load(&paths.config_file())?;

    if let Some(home) = path {
        let runtime = java::detect_any(Some(&home), paths)
            .ok_or_else(|| anyhow::anyhow!("no Java found at {}", home.display()))?;
        config.java_home = Some(runtime.home.clone());
        config.save(&paths.config_file())?;
        println!(
            "java: {} ({})",
            runtime.java.display(),
            runtime.version
        );
        println!("saved java_home: {}", runtime.home.display());
        if runtime.major < 25 {
            println!("warning: Horizon 26.3 needs Java 25");
        }
        return Ok(());
    }

    match java::detect_any(config.java_home.as_deref(), paths) {
        Some(runtime) => {
            println!(
                "java: {} ({})",
                runtime.java.display(),
                runtime.version
            );
            println!("java_home: {}", runtime.home.display());
        }
        None => println!("java: not found"),
    }
    Ok(())
}

fn prepare_command(
    paths: &AppPaths,
    version: Option<String>,
    force: bool,
    java_home: Option<PathBuf>,
) -> anyhow::Result<()> {
    paths.ensure_layout()?;
    let mut config = Config::load(&paths.config_file())?;
    let tag = version
        .or_else(|| config.selected_version.clone())
        .unwrap_or_else(|| github_releases::DEFAULT_TAG.to_owned());
    github_releases::validate_tag(&tag)?;

    let api = prepare::launch_api(paths, &tag)?;
    println!("horizon: {} (launch API {})", api.horizon, api.launch_api);
    println!("minecraft: {} (Java {})", api.minecraft, api.java_major);

    if !prepare::minecraft_ready(paths, &api.minecraft) {
        println!(
            "Minecraft {} files are missing; downloading them from Mojang",
            api.minecraft
        );
        minecraft(paths, &api.minecraft)?;
    }

    let java_home = java_home.or(config.java_home.clone());
    let before = prepare::check(paths, &tag)?;
    if before.ok && !force {
        println!(
            "resources already ready: {}",
            paths.resources_dir_for(&tag).display()
        );
        config.selected_version = Some(tag);
        config.save(&paths.config_file())?;
        return Ok(());
    }
    if !before.ok && !before.details.is_empty() {
        println!("check before preparation:");
        println!("{}", before.details);
    }

    let cancel = AtomicBool::new(false);
    let mut last_step = 0u32;
    let prepared = prepare::prepare(
        paths,
        &tag,
        java_home.as_deref(),
        &cancel,
        |event| match event {
            PrepareProgress::Message(message) => println!("{message}"),
            PrepareProgress::Log(line) => println!("  {line}"),
            PrepareProgress::Fraction(fraction) => {
                let percent = (fraction * 100.0) as u32;
                if percent / 5 != last_step / 5 {
                    last_step = percent;
                    println!("  progress: {percent}%");
                }
            }
        },
    )?;

    let after = prepare::check(paths, &tag)?;
    if !after.ok {
        anyhow::bail!(
            "resources still incomplete after preparation:\n{}",
            after.details
        );
    }

    config.selected_version = Some(tag.clone());
    config.save(&paths.config_file())?;
    println!(
        "prepared: {} (Horizon {}, Minecraft {}, Java {})",
        prepared.output.display(),
        prepared.horizon,
        prepared.minecraft_version,
        prepared.java_major
    );
    Ok(())
}

fn instance_command(paths: &AppPaths, action: InstanceCommand) -> anyhow::Result<()> {
    paths.ensure_layout()?;
    match action {
        InstanceCommand::List => {
            let instances = instances::all(paths)?;
            if instances.is_empty() {
                println!("no instances (run `rustcraft instance create <NAME>`)");
                return Ok(());
            }
            let config = Config::load(&paths.config_file())?;
            for instance in instances {
                let marker = if config.selected_instance.as_deref() == Some(&instance.id) {
                    "*"
                } else {
                    " "
                };
                println!(
                    "{marker} {}\t{}\t{}\t{}",
                    instance.id,
                    instance.name,
                    instance.version_tag,
                    paths.instance_dir(&instance.id).display()
                );
            }
            Ok(())
        }
        InstanceCommand::Create { name, version } => {
            let config = Config::load(&paths.config_file())?;
            let tag = match version {
                Some(tag) => {
                    github_releases::validate_tag(&tag)?;
                    tag
                }
                None => selected_tag(paths, &config)?,
            };
            if !paths.version_dir(&tag).is_dir() {
                anyhow::bail!(
                    "Horizon {} is not installed; run `rustcraft install {tag}`",
                    tag
                );
            }
            let instance = Instance::create(paths, &name, &tag)?;
            println!(
                "created: {} ({}) for Horizon {}",
                instance.name,
                instance.id,
                instance.version_tag
            );
            println!("folder: {}", paths.instance_dir(&instance.id).display());
            Ok(())
        }
        InstanceCommand::Remove { instance } => {
            let Some(found) = instances::find(paths, &instance)? else {
                anyhow::bail!("instance not found: {instance}");
            };
            let dir = paths.instance_dir(&found.id);
            std::fs::remove_dir_all(&dir).map_err(|e| {
                anyhow::anyhow!("could not remove {}: {e}", dir.display())
            })?;
            println!("removed: {} ({})", found.name, found.id);
            Ok(())
        }
    }
}

fn play(
    paths: &AppPaths,
    instance: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
) -> anyhow::Result<()> {
    paths.ensure_layout()?;
    let mut config = Config::load(&paths.config_file())?;

    let name = config.player_name.clone().ok_or_else(|| {
        anyhow::anyhow!("no player name saved; run `rustcraft name <NAME>` first")
    })?;
    let account = OfflineAccount::new(name)?;

    let tag = selected_tag(paths, &config)?;
    let instance = match instance {
        Some(who) => instances::find(paths, &who)?
            .ok_or_else(|| anyhow::anyhow!("instance not found: {who}"))?,
        None => match config
            .selected_instance
            .as_deref()
            .map(|id| instances::load(paths, id))
            .transpose()?
            .flatten()
        {
            Some(found) => found,
            None => instances::ensure_default(paths, &tag)?,
        },
    };

    let width = width.unwrap_or(config.window.width).clamp(320, 7680);
    let height = height.unwrap_or(config.window.height).clamp(240, 4320);

    // Instances created before a version was chosen can point at a tag that is
    // not installed. Rebind them to the selected or newest installed version.
    let mut instance = instance;
    let installed = github_releases::installed_versions(paths)?;
    let is_installed = |tag: &str| installed.iter().any(|version| version.meta.tag == tag);
    if !is_installed(&instance.version_tag) {
        if let Some(fallback) = installed
            .iter()
            .find(|version| version.meta.tag == tag)
            .or_else(|| installed.first())
        {
            instance.version_tag = fallback.meta.tag.clone();
            instance.save(paths)?;
        }
    }

    println!(
        "launching {} ({}) as {} [{}]",
        instance.name, instance.version_tag, account.name, account.uuid
    );
    println!("game dir: {}", paths.instance_dir(&instance.id).display());

    let cancel = AtomicBool::new(false);
    let outcome = launch::run(
        paths,
        &instance,
        &account,
        width,
        height,
        &cancel,
        |event| match event {
            launch::Event::Stdout(line) => println!("{line}"),
            launch::Event::Stderr(line) => println!("{line}"),
        },
    )?;

    println!("game exited with code {}", outcome.code);
    if let Some(report) = &outcome.crash_report {
        println!("crash report: {}", report.display());
        if let Ok(text) = std::fs::read_to_string(report) {
            for line in text.lines().take(20) {
                println!("  {line}");
            }
        }
    }

    if let Some(mut saved) = instances::load(paths, &instance.id)? {
        saved.mark_played(paths)?;
    }
    config.selected_instance = Some(instance.id);
    config.selected_version = Some(instance.version_tag);
    config.save(&paths.config_file())?;
    Ok(())
}

fn selected_tag(paths: &AppPaths, config: &Config) -> anyhow::Result<String> {
    if let Some(tag) = config.selected_version.clone() {
        return Ok(tag);
    }
    github_releases::installed_versions(paths)?
        .into_iter()
        .next()
        .map(|version| version.meta.tag)
        .ok_or_else(|| {
            anyhow::anyhow!("no Horizon version installed; run `rustcraft install`")
        })
}
