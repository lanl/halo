// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use clap::Args;

use std::{
    error::Error,
    io::{Read, Write},
    process::{Command, Stdio},
};

use crate::{handled_error, host::*, HandledResult};

#[derive(Args, Debug, Clone)]
pub struct PowerArgs {
    /// The location of the config file.
    #[arg(long)]
    pub config: Option<String>,

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

pub fn power(args: &PowerArgs) -> HandledResult<()> {
    if args.hostnames.is_empty() {
        return status_all_hosts_in_config(args);
    }

    if let Some(fence_agent) = args.fence_agent.as_ref() {
        return do_fence_given_agent(fence_agent, args);
    }

    // If the user has not specified a fence agent, then assume that the fence parameters for the
    // requested host(s) are found in the config file.

    let config = crate::config::Config::from_file(args.config.as_deref())?;

    for hostname in args.hostnames.iter() {
        let host = config.get_host(hostname).unwrap();

        let agent = FenceAgent::from_config(host).unwrap();
        // The host name might have a ":port" suffix; remove that.
        let hostname = host.hostname.split(":").next().unwrap();

        match do_fence(hostname, &agent, args.action) {
            Ok(()) => {
                eprintln!("{hostname} Fence: Success");
            }
            Err(e) => {
                eprintln!("{hostname} Fence result: Failure: {e}");
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

    let mut error_seen = false;

    for host in &args.hostnames {
        if args.verbose {
            eprintln!("Fencing Host: {}", host);
        }

        match do_fence(host, &fence_agent, args.action) {
            Ok(()) => {
                eprintln!("{} Fence: Success", host);
            }
            Err(e) => {
                eprintln!("{} Fence result: Failure: {e}", host);
                error_seen = true;
            }
        }
    }

    if error_seen {
        handled_error()
    } else {
        Ok(())
    }
}

/// When no hostnames are specified, it is assumed that the user is requesting the power status of
/// every host in the config.
fn status_all_hosts_in_config(args: &PowerArgs) -> HandledResult<()> {
    match &args.action {
        FenceCommand::Status => {}
        other => {
            eprintln!("Must specify host names to perform action \"{other}\".");
            return handled_error();
        }
    };

    let config = crate::config::Config::from_file(args.config.as_deref())?;

    for host in &config.hosts {
        let agent = FenceAgent::from_config(host).unwrap();

        // The host name might have a ":port" suffix; remove that.
        let hostname = host.hostname.split(":").next().unwrap();

        match is_host_powered_on(hostname, agent) {
            Ok(true) => println!("{} is on", hostname),
            Ok(false) => println!("{} is off", hostname),
            Err(e) => println!("Could not determine power status for {}: {e}", hostname),
        }
    }

    Ok(())
}

/// Attempt to check this host's power status.
///
/// If self.fence_agent is not set, then panics.
fn is_host_powered_on(hostname: &str, agent: FenceAgent) -> Result<bool, Box<dyn Error>> {
    let mut child = Command::new(agent.get_executable())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;

    let command_bytes = agent.generate_command_bytes(hostname, FenceCommand::Status);

    child
        .stdin
        .as_mut()
        .expect("stdin should have been captured")
        .write_all(&command_bytes)?;
    let status = child.wait()?;

    if !status.success() {
        return Err(Box::new(power::FenceError {}));
    }

    let mut out = String::new();
    child.stdout.unwrap().read_to_string(&mut out)?;

    if out.contains("is ON") {
        Ok(true)
    } else if out.contains("is OFF") {
        Ok(false)
    } else {
        Err(Box::new(power::FenceError {}))
    }
}

/// Attempt to power on or off this host.
///
/// If self.fence_agent is not set, then panics.
///
/// This is the blocking variant - it is safe to use in commands, but should not be called from
/// the management service.
fn do_fence(
    hostname: &str,
    agent: &FenceAgent,
    command: FenceCommand,
) -> Result<(), Box<dyn Error>> {
    if matches!(command, FenceCommand::Status) {
        panic!("Please use is_powered_on() for power status.");
    }

    let mut child = Command::new(agent.get_executable())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;

    let command_bytes = agent.generate_command_bytes(hostname, command);

    child
        .stdin
        .as_mut()
        .expect("stdin should have been captured")
        .write_all(&command_bytes)?;
    let status = child.wait()?;

    let mut out = String::new();
    child.stdout.unwrap().read_to_string(&mut out)?;
    log::debug!("out: {out}");

    if status.success() {
        Ok(())
    } else {
        Err(Box::new(power::FenceError {}))
    }
}
