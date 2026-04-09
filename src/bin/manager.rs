// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use clap::Parser;

use halo_lib::{self, cluster, manager};

/// The halo_manager binary runs the management daemon.
fn main() {
    let mut args = manager::Cli::parse();

    // If no statefile argument was passed, use the default statefile path.
    // This is done because it's not permitted to start the manager without a statefile.
    //
    // However, the constructor Cluster::new() cannot interpret None as "use the default statefile
    // path" because some callers of Cluster::new() do not start the manager process and therefore
    // have no need of a statefile. So this has to be done here.
    if args.statefile.is_none() {
        args.statefile = Some(halo_lib::default_statefile_path());
    }

    let default_log_level = if args.verbose { "debug" } else { "warn" };
    env_logger::Builder::from_env(
        env_logger::Env::default().filter_or("HALO_LOG", default_log_level),
    )
    .init();
    let Ok(cluster) = cluster::Cluster::new(args) else {
        std::process::exit(1);
    };

    if manager::main(cluster).is_err() {
        std::process::exit(1);
    }
}
