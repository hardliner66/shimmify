use std::{
    collections::BTreeMap,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, exit},
};

use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use thiserror::Error;

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
        /// The name of the shim to remove
        name: String,
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
    #[command(aliases = ["r", "run"])]
    Run {
        /// The arguments the shim gets passed
        #[arg(trailing_var_arg = true)]
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
    #[error("Shim already exists! Use --force to overwrite.")]
    ShimAlreadyExists,
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
        let config_path = config_path.unwrap_or_else(|| default_config.as_ref().to_path_buf());
        let mut config = if let Ok(config) = std::fs::read_to_string(&config_path) {
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

        if let ShimmifyResult::Stop =
            config.handle(config_path, action.unwrap_or_default(), &services)?
        {
            exit(0);
        }
        Ok(())
    }
}

#[derive(Default, Serialize, Deserialize)]
struct ShimmifyConfig {
    current: Option<String>,
    shims: BTreeMap<String, PathBuf>,
}

enum ShimmifyResult {
    Continue,
    Stop,
}

impl ShimmifyConfig {
    fn handle(
        &mut self,
        config_path: impl AsRef<Path>,
        action: ShimmifyAction,
        services_to_restart: &[&str],
    ) -> Result<ShimmifyResult, ShimmifyError> {
        let mut restart_needed = false;
        match action {
            ShimmifyAction::Add {
                name,
                path,
                use_shim,
                force,
            } => {
                let old = self.shims.insert(name.clone(), path.canonicalize()?);
                if old.is_some() && !force {
                    return Err(ShimmifyError::ShimAlreadyExists);
                }
                if use_shim {
                    return self.handle(
                        config_path,
                        ShimmifyAction::Use { name },
                        services_to_restart,
                    );
                }
            }
            ShimmifyAction::Remove { name } => {
                let old = self.shims.remove(&name);
                if old.is_none() {
                    eprintln!("Shim does not exist!");
                    return Ok(ShimmifyResult::Stop);
                }
                if self.current.as_deref() == Some(&name) {
                    self.current = None;
                    restart_needed = true;
                }
            }
            ShimmifyAction::Use { name } => {
                if !self.shims.contains_key(&name) {
                    eprintln!("Shim does not exist!");
                    return Ok(ShimmifyResult::Stop);
                }
                if self.current.as_ref() != Some(&name) {
                    self.current = Some(name);
                    restart_needed = true;
                }
            }
            ShimmifyAction::Reset => {
                self.current = None;
                restart_needed = true;
            }
            ShimmifyAction::Run { args } => {
                let bin = match &self.current {
                    Some(name) => {
                        let Some(path) = self.shims.get(name) else {
                            eprintln!("Shim does not exist!");
                            return Ok(ShimmifyResult::Stop);
                        };
                        path
                    }
                    None => return Ok(ShimmifyResult::Continue),
                };
                return Err(std::process::Command::new(bin)
                    .envs(std::env::vars())
                    .args(&args)
                    .exec()
                    .into());
            }
            ShimmifyAction::List => {
                if self.shims.is_empty() {
                    println!("No shims configured!");
                } else {
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
                }
                return Ok(ShimmifyResult::Stop);
            }
        }

        std::fs::write(config_path, toml::to_string_pretty(self)?)?;

        if restart_needed && !services_to_restart.is_empty() {
            return Err(Command::new("systemctl")
                .arg("restart")
                .args(services_to_restart)
                .exec()
                .into());
        }

        Ok(ShimmifyResult::Stop)
    }
}
