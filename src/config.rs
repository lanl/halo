// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{handled_error, Handle, HandledResult};

#[derive(Serialize, Deserialize, Debug)]
pub struct Config {
    pub hosts: Vec<Host>,
    pub resources: Vec<Resource>,
    pub resource_groups: Vec<ResourceGroup>,
}

impl Config {
    pub fn from_file(path: Option<&str>) -> HandledResult<Self> {
        let path = match path {
            Some(path) => path,
            None => &crate::default_config_path(),
        };

        let config = std::fs::read_to_string(path).handle_err(|e| {
            eprintln!("Could not open config file \"{path}\": {e}");
        })?;

        let config: crate::config::Config = serde_yaml::from_str(&config).handle_err(|e| {
            eprintln!("Could not parse config file \"{path}\": {e}");
        })?;

        Ok(config)
    }

    pub fn get_resource(&self, name: &str) -> &Resource {
        for res in &self.resources {
            if res.name == name {
                return res;
            }
        }

        panic!("Resource {name} referenced but not defined anywhere. Invalid config.");
    }

    pub fn get_host(&self, name: &str) -> Option<&Host> {
        self.hosts
            .iter()
            .find(|h| h.hostname.split(":").next().unwrap() == name)
    }

    /// Get all the failover pairs from this config.
    pub fn get_failover_partners(&self) -> HashMap<String, String> {
        self.resource_groups
            .iter()
            .flat_map(|rg| {
                let Some(partner) = rg.failover_hosts.first() else {
                    return vec![];
                };

                vec![
                    (rg.home_host.clone(), partner.clone()),
                    (partner.clone(), rg.home_host.clone()),
                ]
            })
            .collect()
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Host {
    /// Hostname or IP address, with an optional port suffix.
    pub hostname: String,

    /// Name of the fence agent executable to use for fencing this host.
    pub fence_agent: Option<String>,

    /// Fence parameters for this host.
    pub fence_parameters: Option<HashMap<String, String>>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ResourceGroup {
    pub root: String,
    pub home_host: String,
    pub failover_hosts: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Resource {
    pub name: String,
    pub kind: String,

    pub parameters: HashMap<String, String>,

    pub dependents: Vec<String>,
}

impl Resource {
    pub fn new_zpool(pool: String) -> Self {
        Self {
            name: pool.clone(),
            kind: "heartbeat/ZFS".to_string(),
            parameters: HashMap::from([("pool".to_string(), pool)]),
            dependents: vec![],
        }
    }

    /// Given a line of output from the `mount` command, parses it into a Lustre Resource.
    pub fn new_lustre(mount_output: &str) -> HandledResult<Self> {
        let mut tokens = mount_output.split_whitespace();

        let device = tokens.next().unwrap();
        let mountpoint = tokens.nth(1).unwrap();

        let opts = tokens.nth(2).unwrap();
        let opts = opts.trim_matches(|c| c == '(' || c == ')').split(',');
        let mut kind: Option<String> = None;
        for opt in opts {
            if opt.starts_with("svname=") {
                if opt.contains("MDT") {
                    kind = Some("mdt".to_string());
                } else if opt.contains("MGS") {
                    kind = Some("mgs".to_string());
                } else if opt.contains("OST") {
                    kind = Some("ost".to_string());
                }
            }
        }
        let Some(kind) = kind else {
            eprintln!("could not parse lustre mount line: '{mount_output}'");
            return handled_error();
        };
        Ok(Self {
            name: device.to_string(),
            kind: "lustre/Lustre".to_string(),
            parameters: HashMap::from([
                ("mountpoint".to_string(), mountpoint.to_string()),
                ("target".to_string(), device.to_string()),
                ("type".to_string(), kind.to_string()),
            ]),
            dependents: vec![],
        })
    }
}
