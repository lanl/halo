// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use serde::{Deserialize, Serialize};

/// Config, along with its children Node, Target, and Zpool, is the model for a Lustre cluster
/// used in the HALO configuration file. The config file is deserialized into a Config object.
///
/// The model for a cluster used in the config file is intentionally different from the model
/// used to track the status of the cluster in memory. Since they are decoupled, the dynamic
/// model can be changed without needing to change the configuration file format.
#[derive(Serialize, Deserialize, Debug)]
pub struct Config {
    // TODO: update everything to use 'hosts' insead of 'nodes' including Cluster config object
    pub nodes: Vec<Node>,
    pub failover_pairs: Option<Vec<Vec<String>>>,
}

impl Default for Config {
    fn default() -> Self {
        Self::new()
    }
}

impl Config {
    /// configuration file.
    pub fn new() -> Self {
        Config {
            nodes: Vec::new(),
            failover_pairs: None,
        }
    }
    /// Append a Lustre Node to the given Cluster.
    pub fn add_node(&mut self, n: Node) {
        self.nodes.push(n);
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Node {
    pub hostname: String,
    pub zpools: Vec<Zpool>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Zpool {
    pub name: String,
    pub targets: Vec<Target>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Target {
    pub device: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub mountpoint: String,
    pub fstype: String,
}
