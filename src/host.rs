// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::io::{Read, Write};
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
    fence_agent: Option<FenceAgent>,
}

impl Host {
    pub fn new(name: &str, port: Option<u16>, fence_agent: Option<FenceAgent>) -> Self {
        Host {
            id: HostIdentity {
                name: name.to_string(),
                port: match port {
                    Some(p) => p,
                    None => crate::remote_port(),
                },
            },
            status: Mutex::new(HostStatus::Unknown),
            fence_agent,
        }
    }

    /// Create a Host object from a given config::Host object.
    pub fn from_config(config: &crate::config::Host) -> Self {
        let (name, port) = Self::get_host_port(&config.hostname);
        let fence_agent = config
            .fence_agent
            .as_ref()
            .map(|agent| FenceAgent::from_params(agent, &config.fence_parameters));
        Host::new(name, port, fence_agent)
    }

    /// Given a string that may be of the form "<address>:port number>", split it out into the address
    /// and port number portions.
    fn get_host_port(host_str: &str) -> (&str, Option<u16>) {
        let mut split = host_str.split(':');
        let host = split.nth(0).unwrap();
        let port = split.nth(0).map(|port| port.parse::<u16>().unwrap());
        (host, port)
    }

    /// Attempt to power on or off this host.
    ///
    /// If self.fence_agent is not set, then panics.
    pub fn do_fence(&self, command: FenceCommand) -> Result<(), Box<dyn Error>> {
        let agent = self.fence_agent.as_ref().unwrap();

        if matches!(command, FenceCommand::Status) {
            panic!("Please use is_powered_on() for power status.");
        }

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

        let mut out = String::new();
        child.stdout.unwrap().read_to_string(&mut out)?;
        eprintln!("out: {out}");

        if status.success() {
            Ok(())
        } else {
            Err(Box::new(FenceError {}))
        }
    }

    /// Attempt to check this host's power status.
    ///
    /// If self.fence_agent is not set, then panics.
    pub fn is_powered_on(&self) -> Result<bool, Box<dyn Error>> {
        let agent = self.fence_agent.as_ref().unwrap();

        let mut child = Command::new(agent.get_executable())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;

        let command_bytes = agent.generate_command_bytes(&self.id.name, FenceCommand::Status);

        child
            .stdin
            .as_mut()
            .expect("stdin should have been captured")
            .write_all(&command_bytes)?;
        let status = child.wait()?;

        if !status.success() {
            return Err(Box::new(FenceError {}));
        }

        let mut out = String::new();
        child.stdout.unwrap().read_to_string(&mut out)?;
        eprintln!("out: {out}");

        if out.contains("is ON") {
            Ok(true)
        } else if out.contains("is OFF") {
            Ok(false)
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
#[derive(Clone, Copy)]
pub enum FenceCommand {
    On,
    Off,
    Status,
}

impl fmt::Display for FenceCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                FenceCommand::On => "on",
                FenceCommand::Off => "off",
                FenceCommand::Status => "status",
            }
        )
    }
}

/// The list of supported fence agents.
#[derive(Debug, Clone)]
pub enum FenceAgent {
    Powerman,
    Redfish(RedfishArgs),
    Test(TestFenceArgs),
}

impl FenceAgent {
    /// Create a fence agent given the agent name and its configuration parameters.
    ///
    /// The agent name corresponds to the executable file used to run the agent, and the params are
    /// the arguements passed to that executable when running it for a particular host.
    ///
    /// If the given parameters are not valid for the given agent, this panics rather than try to
    /// run with an unusable fence agent. Note that the parameters are not required for powerman,
    /// since the hostname is the only needed parameter, and that is already stored on the Host
    /// object. However, the other fence agents need additional parameters.
    pub fn from_params(agent: &str, params: &Option<HashMap<String, String>>) -> Self {
        if agent == "powerman" {
            return Self::Powerman;
        }

        let params = params
            .as_ref()
            .expect("Could not load config: Fence params are needed but not set.");

        match agent {
            "redfish" => {
                let Some(user) = params.get("username") else {
                    panic!("Redfish username needed but not in config parameters");
                };
                let Some(pass) = params.get("password") else {
                    panic!("Redfish password needed but not in config parameters");
                };
                Self::Redfish(RedfishArgs::new(user.to_string(), pass.to_string()))
            }
            "fence_test" => {
                let Some(args) = TestFenceArgs::new(params) else {
                    panic!("Test fence agent is missing needed parameters");
                };
                Self::Test(args)
            }
            other => {
                panic!("Could not load config: Unknown fence agent \"{other}\".");
            }
        }
    }

    /// Gets the name of the executable file used for a given fence agent.
    fn get_executable(&self) -> &str {
        match self {
            FenceAgent::Powerman => "fence_powerman",
            FenceAgent::Redfish(_) => "fence_redfish",
            FenceAgent::Test(_) => "tests/fence_test",
        }
    }

    /// Fence agents take their arguments on stdin. This function generates the input arguments to
    /// send to a fence agent to do a fence action on the given host.
    fn generate_command_bytes(&self, host_id: &str, command: FenceCommand) -> Vec<u8> {
        let args = match self {
            FenceAgent::Powerman => {
                format!("ipaddr=localhost\naction={0}\nplug={1}\n", command, host_id)
            }
            FenceAgent::Redfish(redfish_args) => format!(
                "ipaddr={0}\naction={1}\nusername={2}\npassword={3}\nssl-insecure=true",
                host_id, command, redfish_args.username, redfish_args.password,
            ),
            FenceAgent::Test(args) => format!(
                "action={}\ntest_id={}\ntarget={}",
                command, args.test_id, args.target
            ),
        };

        eprintln!("{args}");

        args.into_bytes()
    }
}

/// Arguments for the test fence agent
#[derive(Clone, Debug)]
pub struct TestFenceArgs {
    /// The name of the test that this fence agent will run within.
    test_id: String,

    /// The name of the specific remote agent within the test.
    target: String,
}

impl TestFenceArgs {
    pub fn new(params: &HashMap<String, String>) -> Option<Self> {
        let test_id = params.get("test_id")?.to_string();
        let target = params.get("target")?.to_string();

        Some(Self { test_id, target })
    }
}

/// Redfish fence agent arguments.
#[derive(Clone)]
pub struct RedfishArgs {
    username: String,
    password: String,
}

impl RedfishArgs {
    pub fn new(username: String, password: String) -> Self {
        Self { username, password }
    }
}

impl fmt::Debug for RedfishArgs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{{username: {}, password: ***}}", self.username)
    }
}
