// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use {clap::Args, reqwest::StatusCode};

use crate::{commands::*, manager::http};

#[derive(Args, Debug, Clone)]
pub struct FenceArgs {
    /// Host to fence
    #[arg()]
    hostname: String,
}


pub fn fence(cli: &Cli, args: &FenceArgs) -> HandledResult<()>{
    let addr = match &cli.socket {
        Some(s) => s,
        None => &crate::default_socket(),
    };

    do_fence(addr, &args.hostname)
}

pub fn do_fence(addr: &str, hostname: &str) -> HandledResult<()>{
    let params = http::HostArgs {
        command: "fence".into(),
    };

    let do_request = || -> reqwest::Result<_> {
        let client = reqwest::blocking::ClientBuilder::new()
            .unix_socket(addr)
            .build()?;

        client
            .post(format!("http://halo_manager/hosts/{hostname}"))
            .json(&params)
            .send()
    };
    let response = do_request().handle_err(|e| eprintln!("Error making HTTP request: {e}"))?;

    match response.status() {
        StatusCode::OK => return Ok(()),
        StatusCode::NOT_FOUND => {
            eprintln!("Could not fence '{hostname}': host not found.");
        }
        StatusCode::BAD_REQUEST => {
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
