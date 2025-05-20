// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize, Debug)]
pub struct Config {
    pub hosts: Vec<Host>,
    pub failover_pairs: Option<Vec<Vec<String>>>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Host {
    pub hostname: String,

    /// Resources should be given a unique identifier to identify them in this hashmap.
    pub resources: HashMap<String, Resource>,

    /// Name of the fence agent binary to use for fencing this host.
    pub fence_agent: Option<String>,

    /// Fence parameters for this host.
    pub fence_parameters: Option<HashMap<String, String>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Resource {
    /// An OCF Resource Agent identifier, such as "heartbeat/ZFS" or "lustre/Lustre"
    pub kind: String,

    /// The resource parameters, which are to be passed to the OCF Resource Agent.
    pub parameters: HashMap<String, String>,

    /// Each resource is allowed to specify a single dependency. The named resource must be started
    /// before this one.
    pub requires: Option<String>,
}

impl Resource {
    pub fn new_zpool(pool: String) -> Self {
        Self {
            kind: "heartbeat/ZFS".to_string(),
            parameters: HashMap::from([("pool".to_string(), pool)]),
            requires: None,
        }
    }

    /// Given a line of output from the `mount` command, parses it into a Lustre Resource.
    ///
    /// TODO: make this return a result instead of panicking?
    pub fn new_lustre(mount_output: &str) -> Self {
        let mut tokens = mount_output.split_whitespace();

        let device = tokens.next().unwrap();
        let zpool = device.split('/').next().unwrap();
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
            panic!("could not parse lustre mount line")
        };
        Self {
            kind: "lustre/Lustre".to_string(),
            parameters: HashMap::from([
                ("mountpoint".to_string(), mountpoint.to_string()),
                ("target".to_string(), device.to_string()),
                ("kind".to_string(), kind.to_string()),
            ]),
            requires: Some(zpool.to_string()),
        }
    }
}
