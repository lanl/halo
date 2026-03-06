// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use {clap::Args, reqwest::StatusCode};

use crate::{commands::*, manager::http};

#[derive(Args, Debug, Clone)]
pub struct ResetArgs {
    /// Host to activate
    hostname: String,
}

pub fn reset(cli: &Cli, args: &ResetArgs) -> HandledResult<()> {
    let addr = match &cli.socket {
        Some(s) => s,
        None => &crate::default_socket(),
    };

    let hostname = &args.hostname;

    do_reset(addr, hostname)
}

pub fn do_reset(socket_path: &str, hostname: &str) -> HandledResult<()> {
    let params = http::HostArgs {
        command: "reset".to_string(),
        force: None,
    };

    let do_request = || -> reqwest::Result<_> {
        let client = reqwest::blocking::ClientBuilder::new()
            .unix_socket(socket_path)
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
            eprintln!("Could not reset '{hostname}': host not found.");
        }
        StatusCode::CONFLICT => {
            eprint!("Could not reset '{hostname}': ");
            match response.text() {
                Ok(text) => eprintln!("{text}"),
                Err(e) => eprintln!("Error decoding response: {e}"),
            };
        }
        other => {
            eprintln!("Could not reset '{hostname}': unexpected error: {other}");
        }
    }

    handled_error()
}
