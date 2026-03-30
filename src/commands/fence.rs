// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use {clap::Args, reqwest::StatusCode};

use crate::{commands::*, manager::http};

#[derive(Args, Debug, Clone)]
pub struct FenceArgs {
    /// Host to fence
    #[arg()]
    hostname: String,

    #[arg(long)]
    force: bool,
}

pub fn fence(cli: &Cli, args: &FenceArgs) -> HandledResult<()> {
    do_fence(cli.socket.as_deref(), &args.hostname, args.force)
}

pub fn do_fence(addr: Option<&str>, hostname: &str, force_fence: bool) -> HandledResult<()> {
    let params = http::HostArgs {
        command: "fence".into(),
        force: Some(force_fence),
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
            eprintln!("Could not fence '{hostname}': host not found.");
        }
        StatusCode::BAD_REQUEST | StatusCode::CONFLICT => {
            eprint!("Could not fence '{hostname}': ");
            match response.text() {
                Ok(text) => eprintln!("{text}"),
                Err(e) => eprintln!("Error decoding response: {e}"),
            };
        }
        other => {
            eprintln!("Could not fence '{hostname}': unexpected error: {other}");
        }
    }

    handled_error()
}
