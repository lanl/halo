// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use crate::{
    cluster::Cluster,
    commands::{handled_error, Cli, HandledResult},
};

pub fn validate(args: &Cli) -> HandledResult<()> {
    match &args.config {
        Some(config) => {
            let cluster = Cluster::from_config(Some(config.to_string()))?;

            cluster.print_summary();

            Ok(())
        }
        None => {
            eprintln!("Must specify config file using --config.");
            handled_error()
        }
    }
}
