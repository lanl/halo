// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use {clap::Args, reqwest::StatusCode};

use crate::{commands::*, manager::http};

#[derive(Args, Debug, Clone)]
pub struct ManageArgs {
    /// Resource to manage
    resource_id: String,
}

#[derive(Args, Debug, Clone)]
pub struct UnManageArgs {
    /// Resource to unmanage
    resource_id: String,
}

pub fn manage(cli: &Cli, args: &ManageArgs) -> HandledResult<()> {
    send_command(cli.socket.as_deref(), &args.resource_id, true)
}

pub fn unmanage(cli: &Cli, args: &UnManageArgs) -> HandledResult<()> {
    send_command(cli.socket.as_deref(), &args.resource_id, false)
}

pub fn send_command(socket_path: Option<&str>, resource: &str, managed: bool) -> HandledResult<()> {
    let params = http::SetManagedArgs { managed };

    let client = get_http_client(socket_path)?;

    let response = client
        .patch(format!("http://halo_manager/resources/{resource}"))
        .json(&params)
        .send()
        .handle_err(|e| eprintln!("Error making HTTP request: {e}"))?;

    match response.status() {
        StatusCode::OK => Ok(()),
        StatusCode::NOT_FOUND => {
            eprintln!("Could not update '{resource}': resource group not found.");
            eprintln!("Specify root resource ID.");
            handled_error()
        }
        other => {
            eprintln!("Could not update '{resource}': unexpected error: {other}");
            handled_error()
        }
    }
}
