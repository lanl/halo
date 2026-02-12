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

    let cluster = get_status(addr)?;

    for res in cluster.resources {
        if args.exclude_normal && res.status == "Running" {
            continue;
        }

        print!("{}: ", res.status);
        print!("{}\t", res.kind);

        print!("{}\t", res.id);

        if cli.verbose {
            print!(" [");
            let mut params = res.parameters.iter();
            if let Some((key, val)) = params.next() {
                print!("{key}: {val}");
                for (key, val) in params {
                    print!("; {key}: {val}");
                }
            }
            print!("]");
        }

        if let Some(comment) = res.comment {
            print!(" {comment} ");
        }

        if !res.managed {
            print!(" (Unmanaged)");
        }

        println!();
    }

    let mut connected_activated = String::new();
    let mut connected_deactivated = String::new();
    let mut disconnected_activated = String::new();
    let mut disconnected_deactivated = String::new();
    for host in cluster.hosts {
        let node = format!("{},", host.id);
        if host.active {
            if host.connected {
                connected_activated.push_str(&node);
            } else {
                disconnected_activated.push_str(&node);
            }
        } else if host.connected {
            connected_deactivated.push_str(&node);
        } else {
            disconnected_deactivated.push_str(&node);
        }
    }

    let ca: nodeset::NodeSet = connected_activated
        .parse()
        .expect("Unable to parse nodeset from hostnames.");
    let cd: nodeset::NodeSet = connected_deactivated
        .parse()
        .expect("Unable to parse nodeset from hostnames.");
    let da: nodeset::NodeSet = disconnected_activated
        .parse()
        .expect("Unable to parse nodeset from hostnames.");
    let dd: nodeset::NodeSet = disconnected_deactivated
        .parse()
        .expect("Unable to parse nodeset from hostnames.");

    print!("Connected hosts:\t{}", ca);
    if connected_deactivated.is_empty() {
        println!();
    } else {
        println!(", {} (deactivated)", cd);
    }

    print!("Disconnected hosts:\t{}", da);
    if disconnected_deactivated.is_empty() {
        println!();
    } else {
        println!(", {} (deactivated)", dd);
    }

    Ok(())
}

pub fn get_status(socket: &str) -> HandledResult<http::ClusterJson> {
    let do_request = || -> reqwest::Result<http::ClusterJson> {
        let client = reqwest::blocking::ClientBuilder::new()
            .unix_socket(socket)
            .build()?;

        let response = client.get("http://halo_manager/status").send()?;
        response.json()
    };

    do_request().handle_err(|e| eprintln!("Error making HTTP request: {e}"))
}
