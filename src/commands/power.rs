// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use clap::Args;

use std::{
    io::{Read, Write},
    process::{Command, Stdio},
};

use crate::{handled_error, host::*, Handle, HandledResult};

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
    if let Some(fence_agent) = args.fence_agent.as_ref() {
        return do_fence_given_agent(fence_agent, args);
    }

    // If the user has not specified a fence agent, then assume that the fence parameters for the
    // requested host(s) are found in the config file.

    let config = crate::config::Config::from_file(args.config.as_deref())?;

    let hosts: Vec<&crate::config::Host> = if args.hostnames.is_empty() {
        config.hosts.iter().collect()
    } else {
        args.hostnames
            .iter()
            .map(|name| config.get_host(name).unwrap())
            .collect()
    };

    let mut error_seen = false;

    for host in hosts {
        let agent = FenceAgent::from_config(host).unwrap();
        // The host name might have a ":port" suffix; remove that.
        let hostname = host.hostname.split(":").next().unwrap();

        if do_fence(hostname, &agent, args.action).is_err() {
            error_seen = true;
        }
    }

    if error_seen {
        handled_error()
    } else {
        Ok(())
    }
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
            Err(_) => {
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

/// Attempt to check this host's power status.
///
/// If self.fence_agent is not set, then panics.
fn is_host_powered_on(hostname: &str, agent: &FenceAgent) -> HandledResult<bool> {
    let prog = agent.get_executable();

    let (status, child) = run_fence_binary(hostname, agent, FenceCommand::Status)?;

    let mut out = String::new();
    child
        .stdout
        .unwrap()
        .read_to_string(&mut out)
        .handle_err(|e| eprintln!("Could not read stdout of fence binary: {e}"))?;

    let mut err = String::new();
    child
        .stderr
        .unwrap()
        .read_to_string(&mut err)
        .handle_err(|e| eprintln!("Could not read stderr of fence binary: {e}"))?;

    if !status.success() {
        eprintln!("Fence binary '{prog}' failed.");
        eprintln!("stdout: '{out}'");
        eprintln!("stderr: '{err}'");
        return handled_error();
    }

    if out.contains("ON") {
        Ok(true)
    } else if out.contains("OFF") {
        Ok(false)
    } else {
        eprintln!("Fence binary '{prog}' gave unexpected output; cannot determine power status.");
        eprintln!("stdout: '{out}'");
        eprintln!("stderr: '{err}'");
        handled_error()
    }
}

/// Attempt to power on or off this host.
///
/// If self.fence_agent is not set, then panics.
///
/// This is the blocking variant - it is safe to use in commands, but should not be called from
/// the management service.
fn do_fence(hostname: &str, agent: &FenceAgent, command: FenceCommand) -> HandledResult<()> {
    if matches!(command, FenceCommand::Status) {
        return is_host_powered_on(hostname, agent)
            .inspect(|powered_on| {
                println!("{hostname} is {}", if *powered_on { "on" } else { "off" })
            })
            .map(|_| ());
    }

    let (status, child) = run_fence_binary(hostname, agent, command)?;

    let mut out = String::new();
    child
        .stdout
        .unwrap()
        .read_to_string(&mut out)
        .handle_err(|e| eprintln!("Could not read stdout of fence binary: {e}"))?;

    log::debug!("out: {out}");

    if status.success() {
        eprintln!("{hostname} Fence: Success");
        Ok(())
    } else {
        handled_error()
    }
}

fn run_fence_binary(
    hostname: &str,
    agent: &FenceAgent,
    command: FenceCommand,
) -> HandledResult<(std::process::ExitStatus, std::process::Child)> {
    let prog = agent.get_executable();

    let mut child = Command::new(prog)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .handle_err(|e| eprintln!("Could not spawn fence binary '{prog}': {e}"))?;

    let command_bytes = agent.generate_command_bytes(hostname, command);

    child
        .stdin
        .as_mut()
        .expect("stdin should have been captured")
        .write_all(&command_bytes)
        .handle_err(|e| eprintln!("Could not write to stdin of fence binary '{prog}': {e}"))?;

    let status = child
        .wait()
        .handle_err(|e| eprintln!("Could not get exit status of fence binary '{prog}': {e}"))?;

    Ok((status, child))
}
