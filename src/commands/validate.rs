// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use clap::Args;

use crate::{cluster::Cluster, handled_error, HandledResult};

#[derive(Args, Debug, Clone)]
pub struct ValidateArgs {
    /// The location of the config file.
    #[arg(long)]
    pub config: Option<String>,
}

pub fn validate(args: &ValidateArgs) -> HandledResult<()> {
    match &args.config {
        Some(config) => {
            let args = crate::manager::Cli {
                config: Some(config.to_owned()),
                use_insecure_port: true,
                ..Default::default()
            };
            let cluster = Cluster::new(args)?;

            cluster.print_summary();

            Ok(())
        }
        None => {
            eprintln!("Must specify config file using --config.");
            handled_error()
        }
    }
}
