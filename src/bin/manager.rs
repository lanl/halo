// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use clap::Parser;

use halo_lib::{
    self, cluster,
    commands::{self, Cli},
    manager,
};

/// The halo client is used both to launch the monitoring and management daemon,
/// as well as for interactive command line use.
///
/// If launched with no sub-command, the management daemon will run.
///
/// Otherwise, the indicated sub-command will run.
fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().filter_or("HALO_LOG", "warn")).init();

    let args = Cli::parse();

    let res = match &args.command {
        Some(command) => commands::main(&args, command),
        None => {
            let context = manager::MgrContext::new(args);
            let Ok(cluster) = cluster::Cluster::new(std::sync::Arc::new(context)) else {
                std::process::exit(1);
            };
            manager::main(cluster)
        }
    };

    if res.is_err() {
        std::process::exit(1);
    }
}
