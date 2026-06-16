// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use clap::Args;

use crate::{commands::*, manager::http};

#[derive(Args, Debug, Clone)]
pub struct StatusArgs {
    /// Only print abnormal results (resources that are stopped / failed over, etc.)
    #[arg(short = 'x')]
    exclude_normal: bool,

    /// Maximum number of event entries to output.
    #[arg(short = 'e', long, default_value_t = 10)]
    event_count: usize,
}

pub fn status(cli: &Cli, args: &StatusArgs) -> HandledResult<()> {
    let cluster = get_status(cli.socket.as_deref())?;

    for res in cluster.resources {
        if args.exclude_normal && res.status == "Running" {
            continue;
        }

        print!("{:<20}\t", res.id);
        print!("({})\t", res.kind);
        print!("{}", res.status);
        match res.status.as_str() {
            "Running" => print!(" on {}", res.home_host),
            "Running (Failed Over)" => print!(
                " on {}",
                res.failover_host.expect("Failover host must be set here.")
            ),
            _ => {}
        };

        if cli.verbose {
            print!(" [");
            let mut first_one = true;
            let mut params: Vec<_> = res.parameters.keys().collect();
            params.sort();
            for param in params {
                if first_one {
                    first_one = false;
                } else {
                    print!("; ");
                }
                let value = &res.parameters[param];
                print!("{param}: {value}");
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

    println!();

    let mut abnormal = false;
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
                abnormal = true;
                disconnected_activated.push_str(&node);
            }
        } else if host.connected {
            connected_deactivated.push_str(&node);
        } else {
            abnormal = true;
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

    if !args.exclude_normal {
        print!("Connected hosts:\t{}", ca);
        if connected_deactivated.is_empty() {
            println!();
        } else {
            println!(", {} (deactivated)", cd);
        }
    }

    if !args.exclude_normal || abnormal {
        print!("Disconnected hosts:\t{}", da);
        if disconnected_deactivated.is_empty() {
            println!();
        } else {
            println!(", {} (deactivated)", dd);
        }
    }
    if !args.exclude_normal {
        print!("Events: ");
        if cluster.events.is_empty() {
            println!("NULL");
        } else {
            println!();
            for (i, e) in cluster.events.iter().enumerate() {
                if i == args.event_count {
                    break;
                }
                println!("{}", e.syslog_print())
            }
        }
    }
    Ok(())
}

pub fn get_status(socket: Option<&str>) -> HandledResult<http::ClusterJson> {
    let client = get_http_client(socket)?;

    let response = client
        .get("http://halo_manager/status")
        .send()
        .handle_err(|e| eprintln!("Error making HTTP request: {e:?}"))?;

    response
        .json()
        .handle_err(|e| eprintln!("Error decoding JSON: {e}"))
}
