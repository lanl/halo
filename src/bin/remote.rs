// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use clap::Parser;
use halo_lib::remote::{self, Cli};

fn main() {
    let args = Cli::parse();

    if let Err(_) = remote::agent_main(args) {
        std::process::exit(1);
    }
}
