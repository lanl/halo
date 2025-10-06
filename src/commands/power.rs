// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use std::sync::Arc;

use clap::Args;

use crate::{
    commands::{self, Cli, HandledResult},
    host::*,
    manager::MgrContext,
    Cluster,
};

#[derive(Args, Debug, Clone)]
pub struct PowerArgs {
    /// The fencing action to perform.
    action: FenceCommand,

    #[arg()]
    hostnames: Vec<String>,

    #[arg(short, long)]
    verbose: bool,

    /// Fence agent to use, "powerman" or "redfish", case sensitive
    #[arg(short = 'f', long)]
    fence_agent: Option<String>,

    #[arg(short = 'l', long)]
    username: Option<String>,

    #[arg(short = 'p', long)]
    password: Option<String>,
}

pub fn power(main_args: &Cli, args: &PowerArgs) -> HandledResult<()> {
    if args.hostnames.is_empty() {
        return status_all_hosts_in_config(main_args, args);
    }

    if let Some(fence_agent) = args.fence_agent.as_ref() {
        return do_fence_given_agent(fence_agent, args);
    }

    // If the user has not specified a fence agent, then assume that the fence parameters for the
    // requested host(s) are found in the config file.

    let context = Arc::new(MgrContext::new(main_args.clone()));
    let cluster = Cluster::new(context)?;

    for hostname in args.hostnames.iter() {
        let host = cluster.get_host(hostname).unwrap();
        match host.do_fence(args.action) {
            Ok(()) => {
                eprintln!("{} Fence: Success", host.name());
            }
            Err(e) => {
                eprintln!("{} Fence result: Failure: {e}", host.name());
            }
        }
    }

    Ok(())
}

/// Perform a fence action, with the fence agent specified on the command line. In this case, the
/// specified fence agent will override any potential fence agent found in a config file (if a
/// config is passed as an argument.)
fn do_fence_given_agent(fence_agent: &str, args: &PowerArgs) -> HandledResult<()> {
    let fence_agent = match fence_agent {
        "powerman" => FenceAgent::Powerman,
        "redfish" => {
            let user = args.username.clone().unwrap();
            let pass = args.password.clone().unwrap();
            FenceAgent::Redfish(RedfishArgs::new(user, pass))
        }
        other => panic!("unsupported fence agent {other}"),
    };

    let hosts: Vec<Host> = args
        .hostnames
        .iter()
        .map(|host| Host::new(host, None, Some(fence_agent.clone())))
        .collect();

    let mut error_seen = false;

    for host in hosts {
        if args.verbose {
            eprintln!("Fencing Host: {}", host.name());
        }
        match host.do_fence(args.action) {
            Ok(()) => {
                eprintln!("{} Fence: Success", host.name());
            }
            Err(e) => {
                eprintln!("{} Fence result: Failure: {e}", host.name());
                error_seen = true;
            }
        }
    }

    if error_seen {
        commands::handled_error()
    } else {
        Ok(())
    }
}

/// When no hostnames are specified, it is assumed that the user is requesting the power status of
/// every host in the config.
fn status_all_hosts_in_config(main_args: &Cli, args: &PowerArgs) -> HandledResult<()> {
    match &args.action {
        FenceCommand::Status => {}
        other => {
            eprintln!("Must specify host names to perform action \"{other}\".");
            return commands::handled_error();
        }
    };

    let context = Arc::new(MgrContext::new(main_args.clone()));
    let cluster = Cluster::new(context)?;

    for host in cluster.hosts() {
        match host.is_powered_on() {
            Ok(true) => println!("{} is on", host),
            Ok(false) => println!("{} is off", host),
            Err(e) => println!("Could not determine power status for {}, {e}", host),
        }
    }

    Ok(())
}
