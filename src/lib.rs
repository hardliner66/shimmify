use std::{
    collections::BTreeMap,
    env::VarError,
    io::{stderr, stdout},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, exit},
    time::Duration,
};

use clap::{Parser, Subcommand};
use notify::{
    Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
};
use serde::{Deserialize, Serialize};
use shellexpand::path::LookupError;
use signal_hook::{consts::SIGINT, iterator::Signals};
use thiserror::Error;

#[cfg(not(target_family = "unix"))]
compile_error!("shimmify only supports Unix-like systems");

pub fn shimmify(default_config_path: impl AsRef<Path>) {
    if std::env::var("__shimmify_run_unshimmed").is_ok_and(|v| v == "1") {
        return;
    }

    let Args { action, config } = Args::parse();

    let config = config.unwrap_or_else(|| default_config_path.as_ref().to_path_buf());
    let Err(e) = inner(config, action) else {
        exit(0);
    };

    eprintln!("{e}");
    exit(1);
}

fn inner(config_path: impl AsRef<Path>, action: Action) -> Result<(), ShimmifyError> {
    let config_path = shellexpand::path::full(&config_path)?;

    match action {
        Action::Shim { action } => handle_shim_action(config_path, action),
        Action::Exec { args } => exec_shim(config_path, &args),
        Action::Daemon => run_daemon(config_path),
    }
}

#[derive(Default, Subcommand)]
enum ShimAction {
    /// Lists all the configured shims
    #[default]
    #[command(aliases = ["show", "info"])]
    List,
    /// Adds a new shim
    #[command(aliases = ["a", "register", "reg"])]
    Add {
        /// The name of the shim to add
        name: String,
        /// The path to the binary to execute
        path: PathBuf,
        #[arg(short, long = "use")]
        /// Activates the shim after adding it
        use_shim: bool,
        /// Overwrites the shim if it already exists
        #[arg(short, long)]
        force: bool,
    },
    /// Removes a shim
    #[command(aliases = ["d", "delete", "rem"])]
    Remove {
        /// The names of the shims to remove
        #[arg(required_unless_present="all", conflicts_with="all", num_args=1..)]
        names: Vec<String>,

        /// Remove all shims
        #[arg(short, long)]
        all: bool,
    },
    /// Activates a shim
    #[command(aliases = ["u", "activate"])]
    Use {
        /// The name of the shim to use
        name: String,
    },
    /// Deactivates the active shim
    #[command(aliases = ["res", "unset", "deactivate", "default"])]
    Reset,
}

#[derive(Debug, Error)]
enum ShimmifyError {
    #[error("{0}")]
    IoError(#[from] std::io::Error),
    #[error("Shim '{0}' already exists! Use --force to overwrite.")]
    ShimAlreadyExists(String),
    #[error("Shim '{0}' doesnt exists!")]
    ShimDoesNotExists(String),
    #[error("No shims configured!")]
    NoShimsConfigured,
    #[error("Notify error: {0}")]
    Notify(#[from] notify::Error),
    #[error("Error looking up environment variable: {0}")]
    LookUpError(#[from] LookupError<VarError>),
    #[error("{0}")]
    SerializationError(#[from] toml::ser::Error),
    #[error("{0}")]
    DeserializationError(#[from] toml::de::Error),
}

#[derive(Parser)]
struct Args {
    /// The path to the shimmify config file
    #[arg(short, long, global = true, env = "SHIMMIFY_CONFIG")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Starts the binary in daemon mode
    ///
    /// In this mode the binary watches if the active shim changes
    /// and automatically restarts as the new shim
    Daemon,

    /// Executes the active shim
    Exec {
        /// The arguments to pass to the active shim
        #[arg(last = true)]
        args: Vec<String>,
    },

    /// Manages shims (add, remove, use, reset)
    Shim {
        #[command(subcommand)]
        action: Option<ShimAction>,
    },
}

fn handle_shim_action(
    config_path: impl AsRef<Path>,
    action: Option<ShimAction>,
) -> Result<(), ShimmifyError> {
    let mut config = load_config(config_path.as_ref())?;

    config.handle(config_path, action)
}

fn load_config(config_path: &Path) -> Result<ShimmifyConfig, ShimmifyError> {
    match std::fs::read_to_string(config_path) {
        Ok(config) => Ok(toml::from_str(&config)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(ShimmifyConfig::default())
        }
        Err(error) => Err(error.into()),
    }
}

#[derive(Default, Serialize, Deserialize)]
struct ShimmifyConfig {
    #[serde(skip, default)]
    dirty: bool,
    #[serde(skip, default)]
    restart_needed: bool,
    current: Option<String>,
    shims: BTreeMap<String, PathBuf>,
}

impl ShimmifyConfig {
    fn save(&mut self, config_path: impl AsRef<Path>) -> Result<(), ShimmifyError> {
        if self.dirty {
            let config_str = toml::to_string_pretty(self)?;
            if let Some(parent) = config_path.as_ref().parent()
                && !parent.exists()
            {
                std::fs::create_dir_all(parent)?;
            }
            let temporary_path = config_path
                .as_ref()
                .with_extension(format!("tmp-{}", std::process::id()));
            std::fs::write(&temporary_path, config_str)?;
            std::fs::rename(temporary_path, config_path.as_ref())?;
            self.dirty = false;
        }
        Ok(())
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    fn add(
        &mut self,
        name: &str,
        path: impl AsRef<Path>,
        force: bool,
    ) -> Result<(), ShimmifyError> {
        self.mark_dirty();
        let old = self.shims.insert(
            name.to_string(),
            shellexpand::path::full(path.as_ref())?.canonicalize()?,
        );
        if old.is_some() && !force {
            return Err(ShimmifyError::ShimAlreadyExists(name.to_string()));
        }
        println!("Added shim: {name} => {}", path.as_ref().display());
        Ok(())
    }

    fn get_shim(&self) -> Result<PathBuf, ShimmifyError> {
        if let Some(name) = &self.current
            && let Some(bin) = self.shims.get(name)
        {
            return Ok(bin.clone());
        }
        Ok(std::env::current_exe()?)
    }

    fn use_shim(&mut self, name: &str) -> Result<(), ShimmifyError> {
        self.mark_dirty();
        if !self.shims.contains_key(name) {
            return Err(ShimmifyError::ShimDoesNotExists(name.to_string()));
        }
        println!("Active shim: {name}");
        if self.current.as_deref() != Some(name) {
            self.current = Some(name.to_string());
            self.restart_needed = true;
        }
        Ok(())
    }

    fn remove_all(&mut self) {
        self.mark_dirty();
        self.shims.clear();
        if self.current.take().is_some() {
            self.restart_needed = true;
        }
        println!("Removed all shims");
    }

    fn remove(&mut self, names: &[impl AsRef<str>]) -> Result<(), ShimmifyError> {
        self.mark_dirty();
        for name in names {
            let old = self.shims.remove(name.as_ref());
            if old.is_none() {
                return Err(ShimmifyError::ShimDoesNotExists(name.as_ref().to_string()));
            }
            println!("Removed shim: {}", name.as_ref());
            if self.current.as_deref() == Some(name.as_ref()) {
                self.current = None;
                self.restart_needed = true;
            }
        }
        Ok(())
    }

    fn reset(&mut self) {
        self.restart_needed = self.current.take().is_some();
        println!("Reset active shim");
        self.mark_dirty();
    }

    fn list(&self) -> Result<(), ShimmifyError> {
        if self.shims.is_empty() {
            return Err(ShimmifyError::NoShimsConfigured);
        }
        println!("Shims:");
        for (name, path) in &self.shims {
            println!(
                "- {name}{} => {}",
                if self.current.as_deref().is_some_and(|c| c == name) {
                    " (active)"
                } else {
                    ""
                },
                path.display()
            );
        }
        Ok(())
    }

    fn handle(
        &mut self,
        config_path: impl AsRef<Path>,
        action: Option<ShimAction>,
    ) -> Result<(), ShimmifyError> {
        match action.unwrap_or_default() {
            ShimAction::Add {
                name,
                path,
                use_shim,
                force,
            } => {
                self.add(&name, path, force)?;
                if use_shim {
                    self.use_shim(&name)?;
                }
            }
            ShimAction::Remove { all: true, .. } => self.remove_all(),
            ShimAction::Remove { names, .. } => self.remove(&names)?,
            ShimAction::Use { name } => self.use_shim(&name)?,
            ShimAction::Reset => self.reset(),
            ShimAction::List => self.list()?,
        }

        self.save(config_path)?;

        exit(0);
    }
}

fn spawn_shim(shim_path: impl AsRef<Path>) -> Result<std::process::Child, std::io::Error> {
    Command::new(shim_path.as_ref())
        .envs(std::env::vars())
        .env("__shimmify_run_unshimmed", "1")
        .stdout(stdout())
        .stderr(stderr())
        .spawn()
}

fn exec_shim(config_path: impl AsRef<Path>, args: &[String]) -> Result<(), ShimmifyError> {
    let config = load_config(config_path.as_ref())?;
    let shim = config.get_shim()?;
    Err(Command::new(shim)
        .envs(std::env::vars())
        .env("__shimmify_run_unshimmed", "1")
        .args(args)
        .exec()
        .into())
}

fn run_daemon(config_path: impl AsRef<Path>) -> Result<(), ShimmifyError> {
    let mut current_shim = load_config(config_path.as_ref())?.get_shim()?;

    let (watcher_tx, watcher_rx) = std::sync::mpsc::channel();

    let mut watcher = RecommendedWatcher::new(watcher_tx, Config::default())?;

    watcher.watch(config_path.as_ref(), RecursiveMode::NonRecursive)?;

    let mut signals = Signals::new([SIGINT])?;
    let (signal_tx, signal_rx) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        for sig in signals.forever() {
            signal_tx.send(sig).expect("failed to send signal");
        }
    });

    let mut current = spawn_shim(&current_shim)?;
    loop {
        if let Ok(SIGINT) = signal_rx.recv_timeout(Duration::from_millis(500)) {
            let _ = current.kill();
            let _ = current.try_wait();
            exit(0);
        }
        let mut need_update = false;
        if let Ok(Ok(Event {
            kind: EventKind::Modify(_) | EventKind::Create(_),
            paths,
            ..
        })) = watcher_rx.try_recv()
            && let Some(path) = paths.first()
            && let Ok(new_shim) = load_config(path).and_then(|config| config.get_shim())
            && new_shim != current_shim
        {
            current_shim = new_shim;
            need_update = true;
        }
        if need_update {
            let _ = current.kill();
            let _ = current.try_wait();
            current = spawn_shim(&current_shim)?;
        }
    }
}
