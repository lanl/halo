// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use {clap::Args, reqwest::StatusCode};

use crate::{commands::*, manager::http};

#[derive(Args, Debug, Clone)]
pub struct FenceArgs {
    /// Host to fence.
    #[arg()]
    pub hostname: String,

    /// Fence the host even if it was already fenced, without a reset occurring since.
    #[arg(long)]
    pub force: bool,

    /// Reason for fencing the host.
    #[arg(long)]
    pub reason: Option<String>,
}

pub fn fence(cli: &Cli, args: &FenceArgs) -> HandledResult<()> {
    do_fence(cli.socket.as_deref(), args)
}

pub fn do_fence(addr: Option<&str>, args: &FenceArgs) -> HandledResult<()> {
    let hostname = args.hostname.clone();

    let params = http::HostArgs {
        command: "fence".into(),
        force: Some(args.force),
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
            eprintln!("Could not fence '{hostname}': host not found.");
        }
        StatusCode::INTERNAL_SERVER_ERROR | StatusCode::BAD_REQUEST | StatusCode::CONFLICT => {
            eprint!("Will not fence '{hostname}': ");
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
