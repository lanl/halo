// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use {clap::Args, reqwest::StatusCode};

use crate::{commands::*, manager::http};

#[derive(Args, Debug, Clone)]
pub struct ActivateArgs {
    /// Host to activate
    hostname: String,
}

#[derive(Args, Debug, Clone)]
pub struct DeactivateArgs {
    /// Host to deactivate
    hostname: String,
}

pub fn activate(cli: &Cli, args: &ActivateArgs) -> HandledResult<()> {
    do_activate(cli.socket.as_deref(), &args.hostname, true)
}

pub fn deactivate(cli: &Cli, args: &DeactivateArgs) -> HandledResult<()> {
    do_activate(cli.socket.as_deref(), &args.hostname, false)
}

pub fn do_activate(socket_path: Option<&str>, hostname: &str, active: bool) -> HandledResult<()> {
    let params = http::HostArgs {
        command: if active {
            "activate".to_string()
        } else {
            "deactivate".to_string()
        },
        force: None,
    };

    let client = get_http_client(socket_path)?;
    let response = client
        .post(format!("http://halo_manager/hosts/{hostname}"))
        .json(&params)
        .send()
        .handle_err(|e| eprintln!("Error making HTTP request: {e}"))?;

    match response.status() {
        StatusCode::OK => return Ok(()),
        StatusCode::NOT_FOUND => {
            eprintln!(
                "Could not {}activate '{hostname}': host not found.",
                if active { "" } else { "de" }
            );
        }
        StatusCode::BAD_REQUEST | StatusCode::CONFLICT => {
            eprint!(
                "Could not {}activate '{hostname}': ",
                if active { "" } else { "de" }
            );
            match response.text() {
                Ok(text) => eprintln!("{text}"),
                Err(e) => eprintln!("Error decoding response: {e}"),
            };
        }
        other => {
            eprintln!(
                "Could not {}activate '{hostname}': unexpected error: {other}",
                if active { "" } else { "de" }
            );
        }
    }

    handled_error()
}
