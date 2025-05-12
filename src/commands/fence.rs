// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use clap::Args;

use crate::host::*;

#[derive(Args, Debug)]
pub struct FenceArgs {
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
    redfish_username: Option<String>,

    #[arg(short = 'p', long)]
    redfish_password: Option<String>,
}

pub fn fence(args: &FenceArgs) {
    let hosts: Vec<Host> = args
        .hostnames
        .iter()
        .map(|host| Host::new(host, None))
        .collect();

    if args.fence_agent == "redfifsh"
        && (args.redfish_username.is_none() || args.redfish_password.is_none())
    {
        panic!("No redfish user and or password passed, Exiting.");
    }

    for host in hosts {
        if args.verbose {
            eprintln!("Fencing Host: {}", host.name());
        }
        match host.do_fence(
            match args.action.as_str() {
                "On" => FenceCommand::On,
                "Off" => FenceCommand::Off,
                _ => panic!("Invalid fencing command passed, Cannot continue, Exiting."),
            },
            match args.fence_agent.as_str() {
                "powerman" => FenceAgent::PowerMan,
                "redfish" => FenceAgent::RedFish(RedFishArgs::new(
                    args.redfish_username.clone().unwrap(),
                    args.redfish_password.clone().unwrap(),
                )),
                _ => panic!("Invalid fence agent passed, Cannot continue, Exiting."),
            },
        ) {
            Ok(()) => {
                eprintln!("{} Fence: Success", host.name());
            }
            Err(e) => {
                eprintln!("{} Fence result: Failure: {e}", host.name());
            }
        }
    }
}
