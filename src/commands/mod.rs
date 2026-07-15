// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

pub mod activate;
pub mod discover;
pub mod events;
pub mod failback;
pub mod fence;
pub mod manage;
pub mod power;
pub mod reset;
pub mod status;
pub mod validate;

use {
    activate::{ActivateArgs, DeactivateArgs},
    discover::DiscoverArgs,
    events::EventsArgs,
    failback::FailbackArgs,
    fence::FenceArgs,
    manage::{ManageArgs, UnManageArgs},
    power::PowerArgs,
    reset::ResetArgs,
    status::StatusArgs,
};

use crate::{Handle, HandledResult};

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Cli {
    #[arg(long, global = true)]
    pub config: Option<String>,

    #[arg(long, global = true)]
    pub statefile: Option<String>,

    #[arg(long, global = true)]
    pub socket: Option<String>,

    #[arg(short, long, global = true)]
    pub verbose: bool,

    #[arg(long)]
    pub mtls: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    Status(StatusArgs),
    Discover(DiscoverArgs),
    Events(EventsArgs),
    Failback(FailbackArgs),
    Fence(FenceArgs),
    Power(PowerArgs),
    Reset(ResetArgs),
    Validate,
    Manage(ManageArgs),
    Unmanage(UnManageArgs),
    Activate(ActivateArgs),
    Deactivate(DeactivateArgs),
}

pub fn main(cli: &Cli) -> HandledResult<()> {
    match &cli.command {
        Commands::Discover(args) => discover::discover(args),
        Commands::Events(args) => events::events(cli, args),
        Commands::Failback(args) => failback::failback(cli, args),
        Commands::Fence(args) => fence::fence(cli, args),
        Commands::Power(args) => power::power(cli, args),
        Commands::Validate => validate::validate(cli),
        Commands::Status(args) => status::status(cli, args),
        Commands::Manage(args) => manage::manage(cli, args),
        Commands::Unmanage(args) => manage::unmanage(cli, args),
        Commands::Activate(args) => activate::activate(cli, args),
        Commands::Deactivate(args) => activate::deactivate(cli, args),
        Commands::Reset(args) => reset::reset(cli, args),
    }
}

/// Convert multiple nodeset strings into a single, deduplicated NodeSet object.
/// A "nodeset" is a string representing shorthand notation for a group of hosts (e.g.,
/// 'node[00-05]').
fn merge_nodesets(nodesets: &[String]) -> Result<nodeset::NodeSet, nodeset::NodeSetParseError> {
    let mut nodeset = nodeset::NodeSet::new();
    for nodeset_str in nodesets.iter() {
        let curr_nodeset = &nodeset_str.parse()?;
        nodeset = nodeset.union(curr_nodeset);
    }
    Ok(nodeset)
}

/// Convert multiple nodesets into a vector of hostname strings.
fn nodesets2hostnames(nodesets: &[String]) -> Result<Vec<String>, nodeset::NodeSetParseError> {
    Ok(merge_nodesets(nodesets)?.iter().collect())
}

fn get_http_client(socket_path: Option<&str>) -> HandledResult<reqwest::blocking::Client> {
    let addr = match socket_path {
        Some(s) => s,
        None => &crate::default_socket(),
    };

    let client = reqwest::blocking::ClientBuilder::new()
        .unix_socket(addr)
        .build()
        .handle_err(|e| eprintln!("Could not create HTTP client at {}: {}", addr, e))?;

    Ok(client)
}
