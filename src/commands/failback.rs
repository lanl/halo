// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use {clap::Args, reqwest::StatusCode};

use crate::{commands::*, manager::http};

#[derive(Args, Debug, Clone)]
pub struct FailbackArgs {
    /// Host to failback onto
    #[arg(long = "onto")]
    pub hostname: String,

    /// Reason for performing failback operation.
    #[arg(long)]
    pub reason: Option<String>,
}

pub fn failback(cli: &Cli, args: &FailbackArgs) -> HandledResult<()> {
    do_failback(cli.socket.as_deref(), args)
}

pub fn do_failback(addr: Option<&str>, args: &FailbackArgs) -> HandledResult<()> {
    let hostname = args.hostname.clone();

    let params = http::HostArgs {
        command: "failback".into(),
        force: None,
        comment: args.reason.clone(),
    };

    let client = get_http_client(addr)?;
    let response = client
        .post(format!("http://halo_manager/hosts/{hostname}"))
        .json(&params)
        .send()
        .handle_err(|e| eprintln!("Error making HTTP request: {e}"))?;

    match response.status() {
        StatusCode::OK => return Ok(()),
        StatusCode::NOT_FOUND => {
            eprintln!("Could not perform failback onto '{hostname}': host not found.");
        }
        StatusCode::BAD_REQUEST => {
            eprint!("Could not perform failback onto '{hostname}': ");
            match response.text() {
                Ok(text) => eprintln!("{text}"),
                Err(e) => eprintln!("Error decoding response: {e}"),
            };
        }
        other => {
            eprintln!("Could not perform failback onto '{hostname}': unexpected error: {other}");
        }
    }

    handled_error()
}
