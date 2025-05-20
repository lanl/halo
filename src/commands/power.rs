// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use clap::Args;

use crate::host::*;

#[derive(Args, Debug)]
pub struct PowerArgs {
    #[arg()]
    hostnames: Vec<String>,

    #[arg(short, long)]
    verbose: bool,

    #[arg(short = 'a', long)]
    action: String,

    /// Fence agent to use, "powerman" or "redfish", case sensitive
    #[arg(short = 'f', long)]
    fence_agent: String,

    #[arg(short = 'l', long)]
    username: Option<String>,

    #[arg(short = 'p', long)]
    password: Option<String>,
}

pub fn power(args: &PowerArgs) {
    let fence_agent = match args.fence_agent.as_str() {
        "powerman" => FenceAgent::Powerman,
        "redfish" => {
            let user = args.username.clone().unwrap();
            let pass = args.password.clone().unwrap();
            FenceAgent::Redfish(RedfishArgs::new(user, pass))
        }
        other => panic!("unsupported fence agent {other}"),
    };

    let command = match args.action.as_str() {
        "on" => FenceCommand::On,
        "off" => FenceCommand::Off,
        other => panic!("Invalid fence command {other}"),
    };

    eprintln!("{:?}", fence_agent);
    let hosts: Vec<Host> = args
        .hostnames
        .iter()
        .map(|host| Host::new(host, None, Some(fence_agent.clone())))
        .collect();

    for host in hosts {
        if args.verbose {
            eprintln!("Fencing Host: {}", host.name());
        }
        match host.do_fence(command) {
            Ok(()) => {
                eprintln!("{} Fence: Success", host.name());
            }
            Err(e) => {
                eprintln!("{} Fence result: Failure: {e}", host.name());
            }
        }
    }
}
