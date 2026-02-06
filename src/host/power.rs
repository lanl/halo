// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use std::{
    collections::HashMap,
    error::Error,
    fmt,
    io::{Read, Write},
    process::{Command, Stdio},
};

use {clap::ValueEnum, log::debug};

#[derive(Debug)]
pub struct FenceError {}

impl fmt::Display for FenceError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "fencing failed")
    }
}

impl Error for FenceError {}

/// The supported fence actions.
#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum FenceCommand {
    On,
    Off,
    Status,
}

impl fmt::Display for FenceCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FenceCommand::On => write!(f, "on"),
            FenceCommand::Off => write!(f, "off"),
            FenceCommand::Status => write!(f, "status"),
        }
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

        args.into_bytes()
    }
}

/// Arguments for the test fence agent
#[derive(Clone, Debug)]
pub struct TestFenceArgs {
    /// The name of the test that this fence agent will run within.
    pub test_id: String,

    /// The name of the specific remote agent within the test.
    pub target: String,
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
    pub username: String,
    pub password: String,
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

impl super::Host {
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

        let command_bytes = agent.generate_command_bytes(&self.address.name, command);

        child
            .stdin
            .as_mut()
            .expect("stdin should have been captured")
            .write_all(&command_bytes)?;
        let status = child.wait()?;

        let mut out = String::new();
        child.stdout.unwrap().read_to_string(&mut out)?;
        debug!("out: {out}");

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

        let command_bytes = agent.generate_command_bytes(&self.address.name, FenceCommand::Status);

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

        if out.contains("is ON") {
            Ok(true)
        } else if out.contains("is OFF") {
            Ok(false)
        } else {
            Err(Box::new(FenceError {}))
        }
    }
}
