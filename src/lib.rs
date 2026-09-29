use std::{
    collections::BTreeMap,
    env::VarError,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, exit},
};

use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use shellexpand::path::LookupError;
use thiserror::Error;

mod expanded_path;

use expanded_path::ExpandedPath;

#[cfg(not(target_family = "unix"))]
compile_error!("shimmify only supports Unix-like systems");

#[derive(Parser)]
pub struct ShimmifyArgs {
    /// The path to the shimmify config file
    #[arg(short, long, global = true, env = "SHIMMIFY_CONFIG")]
    config: Option<PathBuf>,
    /// Restart the associated services when the active shim changes
    #[arg(short, long, global = true)]
    restart: bool,
    /// The action to execute (default: list)
    #[command(subcommand)]
    action: Option<ShimmifyAction>,
}

#[derive(Default, Subcommand)]
pub enum ShimmifyAction {
    /// Adds a new shim
    #[command(aliases = ["a"])]
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
    #[command(aliases = ["u"])]
    Use {
        /// The name of the shim to use
        name: String,
    },
    /// Deactivates the active shim
    #[command(aliases = ["res", "unset"])]
    Reset,
    /// Runs the configured shim, using execvp
    #[command(aliases = ["r"])]
    Run {
        /// The arguments the shim gets passed
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Lists all the configured shims
    #[default]
    #[command(aliases = ["show", "info"])]
    List,
}

#[derive(Debug, Error)]
pub enum ShimmifyError {
    #[error("{0}")]
    IoError(#[from] std::io::Error),
    #[error("Shim '{0}' already exists! Use --force to overwrite.")]
    ShimAlreadyExists(String),
    #[error("Shim '{0}' doesnt exists!")]
    ShimDoesNotExists(String),
    #[error("No shims configured!")]
    NoShimsConfigured,
    #[error("Error looking up environment variable: {0}")]
    LookUpError(#[from] LookupError<VarError>),
    #[error("{0}")]
    SerializationError(#[from] toml::ser::Error),
    #[error("{0}")]
    DeserializationError(#[from] toml::de::Error),
}

impl ShimmifyArgs {
    /// Helper function to handle shimmify args (update config, run shim, etc.)
    ///
    /// # Errors
    /// Returns a `ShimmifyError` if there is an issue reading or writing the configuration file,
    /// or if the requested action cannot be completed.
    pub fn exec(
        self,
        default_config: impl AsRef<Path>,
        services_to_restart: Option<&[&str]>,
    ) -> Result<(), ShimmifyError> {
        let ShimmifyArgs {
            config: config_path,
            action,
            restart,
        } = self;
        let config_path = ExpandedPath::new(
            config_path.unwrap_or_else(|| default_config.as_ref().to_path_buf()),
        )?;
        let mut config = if let Ok(config) = std::fs::read_to_string(config_path.as_path()) {
            toml::from_str(&config)?
        } else {
            ShimmifyConfig::default()
        };

        let services = services_to_restart
            .unwrap_or_default()
            .iter()
            .copied()
            .filter(|_| restart)
            .collect::<Vec<_>>();

        config.handle(&config_path, action.unwrap_or_default(), &services)?;
        Ok(())
    }
}

#[derive(Parser)]
struct InternalArgs {
    #[command(subcommand)]
    action: Option<InternalAction>,
}

#[derive(Subcommand)]
enum InternalAction {
    Shim(ShimmifyArgs),
}

/// Helper function to simplify integration in binaries without cli
pub fn shimmify(default_config: impl AsRef<Path>, services_to_restart: Option<&[&str]>) {
    let InternalArgs {
        action: Some(InternalAction::Shim(shimmify)),
    } = InternalArgs::parse()
    else {
        return;
    };
    if let Err(e) = shimmify.exec(default_config, services_to_restart) {
        eprintln!("{e}");
        exit(1);
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
    fn save(&mut self, config_path: &ExpandedPath) -> Result<(), ShimmifyError> {
        if self.dirty {
            let config_str = toml::to_string_pretty(self)?;
            std::fs::write(config_path.as_path(), config_str)?;
            self.dirty = false;
        }
        Ok(())
    }

    fn restart(&mut self, services_to_restart: &[&str]) -> Result<(), ShimmifyError> {
        if self.restart_needed && !services_to_restart.is_empty() {
            return Err(Command::new("systemctl")
                .arg("restart")
                .args(services_to_restart)
                .exec()
                .into());
        }
        Ok(())
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    fn add(&mut self, name: &str, path: &ExpandedPath, force: bool) -> Result<(), ShimmifyError> {
        self.mark_dirty();
        let old = self
            .shims
            .insert(name.to_string(), path.as_path().canonicalize()?);
        if old.is_some() && !force {
            return Err(ShimmifyError::ShimAlreadyExists(name.to_string()));
        }
        println!("Added shim: {name} => {}", path.as_path().display());
        Ok(())
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

    fn run(&mut self, args: &[impl AsRef<str>]) -> Result<(), ShimmifyError> {
        let Some(name) = &self.current else {
            return Ok(());
        };
        let Some(bin) = self.shims.get(name) else {
            return Err(ShimmifyError::ShimDoesNotExists(name.clone()));
        };
        Err(std::process::Command::new(bin)
            .envs(std::env::vars())
            .args(args.iter().map(std::convert::AsRef::as_ref))
            .exec()
            .into())
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
        config_path: &ExpandedPath,
        action: ShimmifyAction,
        services_to_restart: &[&str],
    ) -> Result<(), ShimmifyError> {
        match action {
            ShimmifyAction::Add {
                name,
                path,
                use_shim,
                force,
            } => {
                self.add(&name, &ExpandedPath::new(path)?, force)?;
                if use_shim {
                    self.use_shim(&name)?;
                }
            }
            ShimmifyAction::Remove { all: true, .. } => self.remove_all(),
            ShimmifyAction::Remove { names, .. } => self.remove(&names)?,
            ShimmifyAction::Use { name } => self.use_shim(&name)?,
            ShimmifyAction::Reset => self.reset(),
            ShimmifyAction::Run { args } => return self.run(&args),
            ShimmifyAction::List => self.list()?,
        }

        self.save(config_path)?;
        self.restart(services_to_restart)?;

        exit(0);
    }
}
