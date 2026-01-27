// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use clap::Args;

use crate::commands::{Cli, HandledResult};

#[derive(Args, Debug, Clone)]
pub struct StatusArgs {
    #[arg(short = 'x')]
    exclude_normal: bool,
}

pub async fn status(_cli: &Cli, _args: &StatusArgs) -> HandledResult<()> {
    todo!()
}
