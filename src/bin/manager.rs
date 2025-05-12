// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use halo_lib::commands::{self, Cli};
use halo_lib::{self, cluster, manager};

use clap::Parser;

/// The halo client is used both to launch the monitoring and management daemon,
/// as well as for interactive command line use.
///
/// If launched with no sub-command, the management daemon will run.
///
/// Otherwise, the indicated sub-command will run.
fn main() {
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

    if let Err(_) = res {
        std::process::exit(1);
    }
}
