// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

pub mod discover;
pub mod power;
pub mod start;
pub mod status;
pub mod stop;
pub mod validate;

use {discover::DiscoverArgs, power::PowerArgs, status::StatusArgs, validate::ValidateArgs};

use clap::{Parser, Subcommand};

use crate::Cluster;

#[derive(Debug)]
pub struct EmptyError {}

use std::fmt;
impl fmt::Display for EmptyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "error")
    }
}

impl<T: std::error::Error> From<T> for EmptyError {
    fn from(_error: T) -> Self {
        EmptyError {}
    }
}

/// Commands use a custom Result type which does not contain any error metadata. This is because
/// the binary's main() function is not supposed to interpret the Result of a command in any way,
/// except to set the exit status.
pub type Result = std::result::Result<(), EmptyError>;

pub fn err() -> Result {
    Result::Err(EmptyError {})
}

#[derive(Parser, Debug, Clone)]
#[command(version, about, long_about = None)]
pub struct Cli {
    #[arg(long, global = true)]
    pub config: Option<String>,

    #[arg(long, global = true)]
    pub socket: Option<String>,

    #[arg(short, long, global = true)]
    pub verbose: bool,

    #[arg(long)]
    pub mtls: bool,

    /// Whether to run in Observe mode (Default, only check on resource status, don't actively
    /// start/stop resources), or Manage mode (actively manage resource state)
    #[arg(long)]
    pub manage_resources: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

impl Default for Cli {
    fn default() -> Self {
        Cli {
            config: Some(crate::default_config_path()),
            socket: Some(crate::default_socket()),
            verbose: false,
            mtls: false,
            manage_resources: false,
            command: None,
        }
    }
}

#[derive(Subcommand, Debug, Clone)]
pub enum Commands {
    Status(StatusArgs),
    Start,
    Stop,
    Discover(DiscoverArgs),
    Power(PowerArgs),
    Validate(ValidateArgs),
}

pub fn main(cli: &Cli, command: &Commands) -> Result {
    if let Commands::Discover(args) = command {
        return discover::discover(args);
    };

    if let Commands::Power(args) = command {
        return power::power(cli, args);
    }

    if let Commands::Validate(args) = command {
        return validate::validate(args);
    }

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let context_arc = std::sync::Arc::new(crate::manager::MgrContext::new(cli.clone()));
        match command {
            Commands::Status(args) => status::status(cli, args).await,
            Commands::Start => {
                let cluster = Cluster::new(context_arc)?;
                start::start(cluster).await
            }
            Commands::Stop => {
                let cluster = Cluster::new(context_arc)?;
                stop::stop(cluster).await
            }
            _ => unreachable!(),
        }
    })
}
