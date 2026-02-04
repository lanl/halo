// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use clap::Parser;

use halo_lib::{self, cluster, manager};

/// The halo_manager binary runs the management daemon.
fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().filter_or("HALO_LOG", "warn")).init();

    let args = manager::Cli::parse();

    let context = manager::MgrContext::new(args);
    let Ok(cluster) = cluster::Cluster::new(std::sync::Arc::new(context)) else {
        std::process::exit(1);
    };

    if manager::main(cluster).is_err() {
        std::process::exit(1);
    }
}
