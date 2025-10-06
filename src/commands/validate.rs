// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use clap::Args;

use crate::{cluster::Cluster, commands::HandledResult};

#[derive(Args, Debug, Clone)]
pub struct ValidateArgs {
    /// The config file to validate.
    #[arg(long)]
    config: String,
}

pub fn validate(args: &ValidateArgs) -> HandledResult<()> {
    let cluster = Cluster::from_config(args.config.to_string())?;

    cluster.print_summary();

    Ok(())
}
