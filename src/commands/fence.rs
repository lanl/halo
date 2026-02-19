// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use {clap::Args, reqwest::StatusCode};

use crate::{commands::*, manager::http};

#[derive(Args, Debug, Clone)]
pub struct FenceArgs {
    /// Host to fence
    #[arg()]
    hostname: String,
    force: bool, 
}

pub fn fence(cli: &Cli, args: &FenceArgs) -> HandledResult<()> {
    let addr = match &cli.socket {
        Some(s) => s,
        None => &crate::default_socket(),
    };

    do_fence(addr, args)
}

pub fn do_fence(addr: &str, fence_args: &FenceArgs) -> HandledResult<()> {
    let params = http::HostArgs {
        command: "fence".into(),
        force_fence: Some(fence_args.force),
    };

    let do_request = || -> reqwest::Result<_> {
        let client = reqwest::blocking::ClientBuilder::new()
            .unix_socket(addr)
            .build()?;

        client
            .post(format!("http://halo_manager/hosts/{}", fence_args.hostname))
            .json(&params)
            .send()
    };
    let response = do_request().handle_err(|e| eprintln!("Error making HTTP request: {e}"))?;

    match response.status() {
        StatusCode::OK => return Ok(()),
        StatusCode::NOT_FOUND => {
            eprintln!("Could not fence '{}': host not found.", fence_args.hostname);
        }
        StatusCode::BAD_REQUEST => {
            eprint!("Could not fence '{}': ", fence_args.hostname);
            match response.text() {
                Ok(text) => eprintln!("{text}"),
                Err(e) => eprintln!("Error decoding response: {e}"),
            };
        }
        other => {
            eprintln!("Could not fence '{}': unexpected error: {other}", fence_args.hostname);
        }
    }

    handled_error()
}
