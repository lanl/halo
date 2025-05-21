// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use std::error::Error;
use std::fmt;
use std::sync::Arc;

use clap::Args;
use clap::ValueEnum;

use crate::commands::Cli;
use crate::host::*;
use crate::manager::MgrContext;
use crate::Cluster;

#[derive(Args, Debug, Clone)]
pub struct PowerArgs {
    /// The fencing action to perform.
    action: PowerAction,

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

#[derive(ValueEnum, Debug, Clone)]
enum PowerAction {
    On,
    Off,
    Status,
}

impl fmt::Display for PowerAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                PowerAction::On => "on",
                PowerAction::Off => "off",
                PowerAction::Status => "status",
            }
        )
    }
}

pub fn power(main_args: &Cli, args: &PowerArgs) -> Result<(), Box<dyn Error>> {
    if args.hostnames.len() == 0 {
        return status_all_hosts_in_config(main_args, args);
    }

    let fence_agent = match args.fence_agent.as_ref().unwrap().as_str() {
        "powerman" => FenceAgent::Powerman,
        "redfish" => {
            let user = args.username.clone().unwrap();
            let pass = args.password.clone().unwrap();
            FenceAgent::Redfish(RedfishArgs::new(user, pass))
        }
        other => panic!("unsupported fence agent {other}"),
    };

    let command = match args.action {
        PowerAction::On => FenceCommand::On,
        PowerAction::Off => FenceCommand::Off,
        PowerAction::Status => FenceCommand::Status,
    };

    let hosts: Vec<Host> = args
        .hostnames
        .iter()
        .map(|host| Host::new(host, None, Some(fence_agent.clone())))
        .collect();

    let mut error_seen: Option<Box<dyn Error>> = None;

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
                error_seen = Some(e);
            }
        }
    }

    if let Some(e) = error_seen {
        return Err(e);
    }

    Ok(())
}

/// When no hostnames are specified, it is assumed that the user is requesting the power status of
/// every host in the config.
fn status_all_hosts_in_config(main_args: &Cli, args: &PowerArgs) -> Result<(), Box<dyn Error>> {
    match &args.action {
        PowerAction::Status => {}
        other => {
            eprintln!("Must specify host names to perform action \"{other}\".");
            return Err(Box::new(FenceError {}));
        }
    };

    let context = Arc::new(MgrContext::new(main_args.clone()));
    let cluster = Cluster::new(context)?;

    for host in cluster.hosts().iter() {
        match host.is_powered_on() {
            Ok(true) => println!("{:?} is on", host),
            Ok(false) => println!("{:?} is off", host),
            Err(e) => println!("Could not determine power status for {:?}, {e}", host),
        }
    }

    Ok(())
}
