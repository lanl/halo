// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

pub mod discover;
pub mod power;
pub mod start;
pub mod status;
pub mod stop;

pub use discover::DiscoverArgs;
pub use power::PowerArgs;
pub use status::StatusArgs;

use clap::{Parser, Subcommand};

use crate::Cluster;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Cli {
    #[arg(long)]
    pub config: Option<String>,

    #[arg(long)]
    pub socket: Option<String>,

    #[arg(short, long)]
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

#[derive(Subcommand, Debug)]
pub enum Commands {
    Status(StatusArgs),
    Start,
    Stop,
    Discover(DiscoverArgs),
    Power(PowerArgs),
}

pub fn main(cli: &Cli, command: &Commands) -> Result<(), Box<dyn std::error::Error>> {
    if let Commands::Discover(args) = command {
        return discover::discover(args);
    };

    if let Commands::Power(args) = command {
        return Ok(power::power(args));
    }

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let context_arc = std::sync::Arc::new(crate::manager::MgrContext::default());
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
