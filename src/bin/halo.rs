// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use clap::Parser;

use halo_lib::{
    self,
    commands::{self, Cli},
};

/// The halo binary is used to launch admin commands like "status", "fence", etc.
fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().filter_or("HALO_LOG", "warn")).init();

    let args = Cli::parse();

    if commands::main(&args).is_err() {
        std::process::exit(1);
    }
}
