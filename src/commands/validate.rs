// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use clap::Args;

use crate::{cluster::Cluster, HandledResult};

#[derive(Args, Debug, Clone)]
pub struct ValidateArgs {
    /// The location of the config file.
    #[arg(long)]
    pub config: String,
}

pub fn validate(args: &ValidateArgs) -> HandledResult<()> {
    let args = crate::manager::Cli {
        config: Some(args.config.to_owned()),
        use_insecure_port: true,
        ..Default::default()
    };
    let cluster = Cluster::new(args)?;

    cluster.print_summary();

    Ok(())
}
