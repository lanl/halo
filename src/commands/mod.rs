// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

pub mod activate;
pub mod discover;
pub mod failback;
pub mod fence;
pub mod manage;
pub mod power;
pub mod start;
pub mod status;
pub mod stop;
pub mod validate;

use {
    activate::{ActivateArgs, DeactivateArgs},
    discover::DiscoverArgs,
    failback::FailbackArgs,
    fence::FenceArgs,
    manage::{ManageArgs, UnManageArgs},
    power::PowerArgs,
    status::StatusArgs,
};

use clap::{Parser, Subcommand};

use crate::cluster::Cluster;

/// A `HandledError` represents an error that has already been handled. When you call a function
/// that returns a `HandledError` or `HandledResult`, you don't need to do anything with that error,
/// other than just be aware that it happened, and return it on to your caller.
///
/// `main()` has a special responsibility: since its "caller" is, in a certain sense, the operating
/// system, `main()` must return a nonzero exit status when it gets a `HandledError`.
///
/// The primary way to construct a `HandledError` is with the `handle_err()` function, which turns a
/// generic error into a `HandledError`, and also runs some caller-provided code to handle the
/// error. That provided code would normally do something like report the error to stderr.
///
/// A `HandledError` inentionally has no data about what the specific error was; the process of
/// handling the error "consumes" that information, and it is no longer needed as the error was
/// already appropriately handled.
#[derive(Debug, PartialEq)]
pub struct HandledError {}

pub type HandledResult<T> = std::result::Result<T, HandledError>;

pub fn handled_error() -> HandledResult<()> {
    HandledResult::Err(HandledError {})
}

pub trait Handle<T, F> {
    fn handle_err(self, handler: F) -> HandledResult<T>;
}

impl<T, E, F: FnOnce(E)> Handle<T, F> for std::result::Result<T, E> {
    /// Handle an error by running the provided `handler` code, giving it the error.
    ///
    /// Then, return a `HandledResult`, so that transitive callers of this function know that they
    /// do not need to do anything further to handle the error.
    fn handle_err(self, handler: F) -> HandledResult<T> {
        self.map_err(|e| {
            handler(e);
            HandledError {}
        })
    }
}

#[derive(Parser, Debug)]
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

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    Status(StatusArgs),
    Start,
    Stop,
    Discover(DiscoverArgs),
    Failback(FailbackArgs),
    Fence(FenceArgs),
    Power(PowerArgs),
    Validate,
    Manage(ManageArgs),
    Unmanage(UnManageArgs),
    Activate(ActivateArgs),
    Deactivate(DeactivateArgs),
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

pub fn main(cli: &Cli) -> HandledResult<()> {
    match &cli.command {
        Commands::Discover(args) => return discover::discover(args),
        Commands::Failback(args) => return failback::failback(cli, args),
        Commands::Fence(args) => return fence::fence(cli, args),
        Commands::Power(args) => return power::power(cli, args),
        Commands::Validate => return validate::validate(cli),
        Commands::Status(args) => return status::status(cli, args),
        Commands::Manage(args) => return manage::manage(cli, args),
        Commands::Unmanage(args) => return manage::unmanage(cli, args),
        Commands::Activate(args) => return activate::activate(cli, args),
        Commands::Deactivate(args) => return activate::deactivate(cli, args),
        _ => {}
    }

    let rt = tokio::runtime::Runtime::new()
        .handle_err(|e| eprintln!("Error launching tokio runtime: {e}"))?;

    rt.block_on(async {
        let cluster = Cluster::from_config(cli.config.clone())?;
        match &cli.command {
            Commands::Start => start::start(cluster).await,
            Commands::Stop => stop::stop(cluster).await,
            _ => unreachable!(),
        }
    })
}
