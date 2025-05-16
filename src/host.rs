// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use std::error::Error;
use std::fmt;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Mutex;

#[derive(Debug, Clone)]
pub struct HostIdentity {
    name: String,
    port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HostStatus {
    Up,
    Down,
    Unknown,
}

/// A server on which services can run.
#[derive(Debug)]
pub struct Host {
    id: HostIdentity,
    status: Mutex<HostStatus>,
}

impl Host {
    pub fn new(name: &str, port: Option<u16>) -> Self {
        Host {
            id: HostIdentity {
                name: name.to_string(),
                port: match port {
                    Some(p) => p,
                    None => crate::remote_port(),
                },
            },
            status: Mutex::new(HostStatus::Unknown),
        }
    }

    /// Create a Host object from a given config::Host object.
    pub fn from_config(host: &crate::config::Host) -> Self {
        let (name, port) = Self::get_host_port(&host.hostname);
        Host::new(name, port)
    }

    /// Given a string that may be of the form "<address>:port number>", split it out into the address
    /// and port number portions.
    fn get_host_port(host_str: &str) -> (&str, Option<u16>) {
        let mut split = host_str.split(':');
        let host = split.nth(0).unwrap();
        let port = split.nth(0).map(|port| port.parse::<u16>().unwrap());
        (host, port)
    }

    pub fn do_fence(&self, command: FenceCommand, agent: FenceAgent) -> Result<(), Box<dyn Error>> {
        let mut child = Command::new(agent.get_executable())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;

        let command_bytes = agent.generate_command_bytes(&self.id.name, command);

        child
            .stdin
            .as_mut()
            .expect("stdin should have been captured")
            .write_all(&command_bytes)?;
        let status = child.wait()?;

        if status.success() {
            Ok(())
        } else {
            Err(Box::new(FenceError {}))
        }
    }

    pub fn get_status(&self) -> HostStatus {
        *self.status.lock().unwrap()
    }

    pub fn set_status(&self, status: HostStatus) {
        match status {
            HostStatus::Down => {
                panic!("Down status for host is not possible yet. (Requires fencing.)");
            }
            _ => {}
        };
        *self.status.lock().unwrap() = status;
    }

    pub fn name(&self) -> &str {
        &self.id.name
    }

    pub fn port(&self) -> u16 {
        self.id.port
    }
}

impl fmt::Display for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.name(), self.port())
    }
}

#[derive(Debug)]
pub struct FenceError {}

impl fmt::Display for FenceError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "fencing failed")
    }
}

impl Error for FenceError {}

/// The supported fence actions.
pub enum FenceCommand {
    On,
    Off,
}

impl fmt::Display for FenceCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", match self {
            FenceCommand::On => "on",
            FenceCommand::Off => "off",
        })
    }
}

/// The list of supported fence agents.
pub enum FenceAgent {
    PowerMan,
    RedFish(RedFishArgs),
}

impl FenceAgent {
    fn get_executable(&self) -> &str {
        match self {
            FenceAgent::PowerMan => "fence_powerman",
            FenceAgent::RedFish(_) => "fence_redfish",
        }
    }

    fn generate_command_bytes(&self, host_id: &str, command: FenceCommand) -> Vec<u8> {
        match self {
            FenceAgent::PowerMan => format!(
                "ipaddr=localhost\naction={0}\nplug={1}\n",
                command,
                host_id
            ),
            FenceAgent::RedFish(redfish_args) => format!(
                "ipaddr={0}\naction={1}\nusername={2}\npassword={3}\nssl-insecure=true",
                host_id,
                command,
                redfish_args.username,
                redfish_args.password,
            ),
        }
        .into_bytes()
    }
}

/// Redfish fence agent arguments.
pub struct RedFishArgs {
    username: String,
    password: String,
}

impl RedFishArgs {
    pub fn new(username: String, password: String) -> Self {
        RedFishArgs { username, password }
    }
}
