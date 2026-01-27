// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use clap::Args;

use crate::commands::{Cli, HandledResult};

#[derive(Args, Debug, Clone)]
pub struct ManageArgs {
    /// Resource to manage
    resource_id: String,
}

#[derive(Args, Debug, Clone)]
pub struct UnManageArgs {
    /// Resource to manage
    resource_id: String,
}

pub async fn manage(cli: &Cli, args: &ManageArgs) -> HandledResult<()> {
    send_command(cli, &args.resource_id, true).await
}

pub async fn unmanage(cli: &Cli, args: &UnManageArgs) -> HandledResult<()> {
    send_command(cli, &args.resource_id, false).await
}

async fn send_command(_cli: &Cli, _resource: &str, _manage: bool) -> HandledResult<()> {
    todo!()
}
