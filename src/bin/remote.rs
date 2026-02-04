// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use clap::Parser;

use halo_lib::remote::{self, Cli};

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().filter_or("HALO_LOG", "warn")).init();

    let args = Cli::parse();

    if remote::agent_main(args).is_err() {
        std::process::exit(1);
    }
}
