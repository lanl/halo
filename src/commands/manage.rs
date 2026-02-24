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
    send_command(&cli.socket, &args.resource_id, true)
}

pub fn unmanage(cli: &Cli, args: &UnManageArgs) -> HandledResult<()> {
    send_command(&cli.socket, &args.resource_id, false)
}

pub fn send_command(
    socket_path: &Option<String>,
    resource: &str,
    managed: bool,
) -> HandledResult<()> {
    let addr = match socket_path {
        Some(s) => s,
        None => &crate::default_socket(),
    };

    let params = http::SetManagedArgs { managed };

    let do_request = || -> reqwest::Result<_> {
        let client = reqwest::blocking::ClientBuilder::new()
            .unix_socket(addr.as_str())
            .build()?;

        client
            .patch(format!("http://halo_manager/resources/{resource}"))
            .json(&params)
            .send()
    };

    let response = do_request().handle_err(|e| eprintln!("Error making HTTP request: {e}"))?;

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
