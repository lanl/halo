// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use clap::Parser;

use halo_lib::remote::{self, Cli};

fn main() {
    let args = Cli::parse();

    let default_log_level = if args.verbose { "debug" } else { "warn" };
    env_logger::Builder::from_env(
        env_logger::Env::default().filter_or("HALO_LOG", default_log_level),
    )
    .init();

    if remote::agent_main(args).is_err() {
        std::process::exit(1);
    }
}
