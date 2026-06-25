// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use clap::Parser;

use halo_lib::{self, cluster, manager};

/// Used to indicate that the manager process exited with an error that requires admin intervention
/// to correct, e.g., an invalid config file, or bad path to the manager socket, etc.
///
/// This is to be distinguished from internal errors (i.e., panics) that should lead systemd to try
/// to restart the manager process.
const EXIT_USER_ERROR: i32 = 64;

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
        std::process::exit(EXIT_USER_ERROR);
    };

    if manager::main(cluster).is_err() {
        std::process::exit(EXIT_USER_ERROR);
    }
}
