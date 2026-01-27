// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use clap::Args;

use crate::{
    commands::{Cli, Handle, HandledResult},
    manager::http,
};

#[derive(Args, Debug, Clone)]
pub struct StatusArgs {
    #[arg(short = 'x')]
    exclude_normal: bool,
}

pub fn status(cli: &Cli, args: &StatusArgs) -> HandledResult<()> {
    let addr = match &cli.socket {
        Some(s) => s,
        None => &crate::default_socket(),
    };

    let do_request = || -> reqwest::Result<http::ClusterJson> {
        let client = reqwest::blocking::ClientBuilder::new()
            .unix_socket(addr.as_str())
            .build()?;

        let response = client.get("http://halo_manager/status").send()?;
        response.json()
    };

    let cluster = do_request().handle_err(|e| eprintln!("Error making HTTP request: {e}"))?;

    for res in cluster.resources {
        if args.exclude_normal && res.status == "Running" {
            continue;
        }

        print!("{}: ", res.status);
        print!("{}\t", res.kind);

        print!(" [");
        let mut params = res.parameters.iter();
        if let Some((key, val)) = params.next() {
            print!("{key}: {val}");
            for (key, val) in params {
                print!("; {key}: {val}");
            }
        }
        print!("]");

        if !res.managed {
            print!(" (Unmanaged)");
        }

        println!();
    }

    Ok(())
}
