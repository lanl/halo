// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use {clap::Args, reqwest::StatusCode};

use crate::{commands::*, manager::http};

#[derive(Args, Debug, Clone)]
pub struct FailbackArgs {
    /// Resource to manage
    #[arg(long = "onto")]
    hostname: String,
}

pub fn failback(cli: &Cli, args: &FailbackArgs) -> HandledResult<()> {
    let addr = match &cli.socket {
        Some(s) => s,
        None => &crate::default_socket(),
    };

    let hostname = &args.hostname;

    do_failback(addr, hostname)
}

pub fn do_failback(addr: &str, hostname: &str) -> HandledResult<()> {
    let params = http::HostArgs {
        command: "failback".into(),
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
