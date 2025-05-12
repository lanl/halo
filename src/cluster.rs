// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use futures::future;
use std::collections::HashMap;
use std::error::Error;
use std::sync::{Arc, Mutex};

use crate::config::*;
use crate::host::*;
use crate::manager::MgrContext;
use crate::resource::*;

/// Cluster is the model used to represent the dynamic state of a cluster in memory.
/// Unlike the persistent model which views a cluster as made up of nodes, which own services,
/// the in-memory model views a cluster as made up of services (storage devices and Lustre
/// targets), and services know which nodes they expect to run on.
///
/// This model is slightly more convenient for performing cluster operations.
#[derive(Debug)]
pub struct Cluster {
    resource_groups: Vec<ResourceGroup>,
    num_zpools: u32,
    num_targets: u32,
    hosts: Vec<Arc<Host>>,
    /// A reference to the shared manager context which contains the verbose output stream and a
    /// copy of the CLI arguments.
    pub context: Arc<MgrContext>,
}

impl Cluster {
    /// Create a new Cluster object given a reference to a MgrContext, which contains the arguments.
    pub fn new(context: Arc<MgrContext>) -> Result<Self, Box<dyn Error>> {
        let path = match &context.args.config {
            Some(path) => path,
            None => &crate::default_config_path(),
        };
        let config = std::fs::read_to_string(path).inspect_err(|e| {
            eprintln!("Could not open config file \"{path}\": {e}");
        })?;
        let config: Config = toml::from_str(&config)?;
        Ok(Self::from_config(&config, context))
    }

    fn from_config(conf: &Config, context: Arc<MgrContext>) -> Self {
        let mut cluster = Cluster {
            resource_groups: Vec::new(),
            hosts: Vec::new(),
            num_zpools: 0,
            num_targets: 0,
            context: Arc::clone(&context),
        };

        for node in conf.nodes.iter() {
            let (hostname, port) = get_host_port(&node.hostname);
            let host = Arc::new(Host::new(hostname, port));
            cluster.hosts.push(Arc::clone(&host));

            for z in node.zpools.iter() {
                let mut zpool_resource = Resource {
                    kind: "heartbeat/ZFS".to_string(),
                    parameters: HashMap::from([("pool".to_string(), z.name.clone())]),
                    dependents: vec![],
                    status: Mutex::new(ResourceStatus::Unknown),
                    home_node: Arc::clone(&host),
                    failover_node: None,
                    context: Arc::clone(&context),
                };
                cluster.num_zpools += 1;

                for target in z.targets.iter() {
                    let target_resource = Resource {
                        kind: "lustre/Lustre".to_string(),
                        parameters: HashMap::from([
                            ("mountpoint".to_string(), target.mountpoint.clone()),
                            ("target".to_string(), target.device.clone()),
                            // "kind" is not actually a needed parameter for the OCF Resource
                            // agent. It's included so that the lustre resources can be treated
                            // differently in this software if is is MGS, MDS, or OST.
                            ("kind".to_string(), target.kind.clone()),
                        ]),
                        dependents: vec![],
                        status: Mutex::new(ResourceStatus::Unknown),
                        home_node: Arc::clone(&host),
                        failover_node: None,
                        context: Arc::clone(&context),
                    };
                    cluster.num_targets += 1;
                    zpool_resource.dependents.push(target_resource);
                }

                let resource_group = ResourceGroup::new(zpool_resource);

                cluster.resource_groups.push(resource_group);
            }
        }

        if let Some(failover_pairs) = &conf.failover_pairs {
            let mut new_groups = Vec::new();
            for group in cluster.resource_groups.iter() {
                let mut new_zpool = Resource {
                    kind: group.root.kind.clone(),
                    parameters: group.root.parameters.clone(),
                    dependents: vec![],
                    status: Mutex::new(ResourceStatus::Unknown),
                    home_node: group.root.home_node.clone(),
                    failover_node: None,
                    context: Arc::clone(&context),
                };
                let hostname: &str =
                    get_failover_partner(&failover_pairs, &group.root.home_node.to_string())
                        .unwrap();
                let host: &Arc<Host> = cluster.get_host_by_name(hostname).unwrap();
                new_zpool.failover_node = Some(Arc::clone(host));

                let mut new_targets = Vec::new();
                for target in &group.root.dependents {
                    let mut new_target = Resource {
                        kind: target.kind.clone(),
                        parameters: target.parameters.clone(),
                        dependents: vec![],
                        status: Mutex::new(ResourceStatus::Unknown),
                        home_node: target.home_node.clone(),
                        failover_node: None,
                        context: Arc::clone(&context),
                    };
                    let hostname: &str =
                        get_failover_partner(&failover_pairs, &target.home_node.to_string())
                            .unwrap();
                    let host: &Arc<Host> = cluster.get_host_by_name(hostname).unwrap();
                    new_target.failover_node = Some(Arc::clone(host));

                    new_targets.push(new_target);
                }

                new_zpool.dependents = new_targets;

                new_groups.push(ResourceGroup::new(new_zpool));
            }
            cluster.resource_groups = new_groups;
        }

        cluster
    }

    /// The main management loop for a cluster consists of running the management loop for each
    /// resource group concurrently.
    pub async fn main_loop(&self) {
        let futures: Vec<_> = self
            .resource_groups
            .iter()
            .map(|r| r.main_loop(&self.context.args))
            .collect();

        let _ = future::join_all(futures).await;
    }

    /// Get reference to host from host string
    fn get_host_by_name(&self, hoststr: &str) -> Option<&Arc<Host>> {
        for host in self.hosts.iter() {
            if host.to_string() == hoststr {
                return Some(&host);
            }
        }
        None
    }

    pub fn num_zpools(&self) -> u32 {
        self.num_zpools
    }

    pub fn num_targets(&self) -> u32 {
        self.num_targets
    }

    pub fn resources(&self) -> impl Iterator<Item = &Resource> {
        self.resource_groups
            .iter()
            .flat_map(|group| group.resources())
    }

    pub fn zpool_resources(&self) -> impl Iterator<Item = &Resource> {
        self.resources().filter(|res| res.kind == "heartbeat/ZFS")
    }

    pub fn lustre_resources(&self) -> impl Iterator<Item = &Resource> {
        self.resources().filter(|res| res.kind == "lustre/Lustre")
    }

    pub fn lustre_resources_no_mgs(&self) -> impl Iterator<Item = &Resource> {
        self.lustre_resources()
            .filter(|res| res.parameters.get("kind").unwrap() != "mgs")
    }

    pub fn get_mgs(&self) -> Option<&Resource> {
        self.lustre_resources()
            .find(|res| res.parameters.get("kind").unwrap() == "mgs")
    }
}

/// Given a string that may be of the form "<address>:port number>", split it out into the address
/// and port number portions.
fn get_host_port(host_str: &str) -> (&str, Option<u16>) {
    let mut split = host_str.split(':');
    let host = split.nth(0).unwrap();
    let port = split.nth(0).map(|port| port.parse::<u16>().unwrap());
    (host, port)
}

/// Given a list `pairs` of failover pairs, and a hostname `name`, return its partner, if one
/// exists.
fn get_failover_partner<'pairs>(
    pairs: &'pairs Vec<Vec<String>>,
    name: &str,
) -> Option<&'pairs str> {
    for pair in pairs.iter() {
        if name == pair[0] {
            return Some(&pair[1]);
        }
        if name == pair[1] {
            return Some(&pair[0]);
        }
    }
    None
}
