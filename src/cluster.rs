// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use std::{collections::HashMap, sync::Arc};

use futures::future;

use crate::{
    commands::{Handle, HandledResult},
    host::*,
    manager,
    resource::*,
    state::{Record, State},
};

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

    /// The hosts in the Cluster are mapped by their ID, a unique identifier which is the hostname
    /// normally. However, in the test environment, it is a test-defined identifier since the
    /// hostname would not be a useful unique ID in the test environment.
    hosts: HashMap<String, Arc<Host>>,

    pub args: manager::Cli,

    /// True if this is a failover cluster (hosts are in high-availability pairs)
    failover: bool,

    /// File to use to store/load cluster state. This must be specified on the manager side where
    /// state must be considered; otherwise, this does not need to be specified.
    state: Option<State>,
}

impl Cluster {
    /// Apply a state Delta to this Cluster's Hosts.
    pub fn apply_state(&self) {
        if let Some(state) = &self.state {
            // Track which hosts have been updated here in order to determine hosts that aren't in
            // the cluster.
            let mut unapplied_hosts: HashMap<String, Option<usize>> = HashMap::new();
            state.delta.hosts().iter().for_each(|v| {
                unapplied_hosts.insert(v.clone(), None);
            });

            for cluster_host in self.hosts() {
                let cluster_host_id = cluster_host.id();

                match state.delta.hosts_fenced.get(&cluster_host_id) {
                    Some(fenced) => {
                        cluster_host.set_fenced(*fenced);
                        unapplied_hosts.remove_entry(&cluster_host_id);
                    }
                    None => {}
                }
                match state.delta.hosts_activated.get(&cluster_host_id) {
                    Some(fenced) => {
                        cluster_host.set_active(*fenced);
                        unapplied_hosts.remove_entry(&cluster_host_id);
                    }
                    None => {}
                }
            }
            for host in unapplied_hosts.keys() {
                eprintln!("host '{host}' not found in cluster");
            }

            // Track which resources have been updated here in order to determine hosts that aren't
            // in the cluster.
            let mut unapplied_resources: HashMap<String, Option<usize>> = HashMap::new();
            state.delta.resources().iter().for_each(|v| {
                unapplied_hosts.insert(v.clone(), None);
            });
            for cluster_rg in self.resource_groups() {
                let cluster_rg_id = cluster_rg.id();

                match state.delta.resources_managed.get(cluster_rg_id) {
                    Some(managed) => {
                        cluster_rg.set_managed(*managed);
                        unapplied_resources.remove_entry(cluster_rg_id);
                    }
                    None => {}
                };
            }
            for rg in unapplied_resources.keys() {
                eprintln!("resource '{rg}' not found in cluster");
            }
        }
    }

    pub async fn main_loop(&self) {
        if self.args.manage_resources {
            if self.failover {
                let futures: Vec<_> = self.hosts.values().map(|h| h.manage_ha(self)).collect();

                let _ = future::join_all(futures).await;
            } else {
                todo!("Implement management loop for non-HA cluster.");
            }
        } else if self.failover {
            let futures: Vec<_> = self.hosts.values().map(|h| h.observe_ha(self)).collect();

            let _ = future::join_all(futures).await;
        } else {
            let futures: Vec<_> = self.hosts.values().map(|h| h.observe(self)).collect();

            let _ = future::join_all(futures).await;
        };
    }

    pub fn num_zpools(&self) -> u32 {
        self.num_zpools
    }

    pub fn num_targets(&self) -> u32 {
        self.num_targets
    }

    pub fn resource_groups(&self) -> impl Iterator<Item = &ResourceGroup> {
        self.resource_groups.iter()
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

    pub fn host_home_resource_groups<'a>(
        &'a self,
        host: &'a Host,
    ) -> impl Iterator<Item = &'a ResourceGroup> {
        self.resource_groups
            .iter()
            .filter(|rg| std::ptr::eq(Arc::as_ptr(rg.home_node()), host))
    }

    pub fn get_resource_group(&self, id: &str) -> &ResourceGroup {
        self.resource_groups
            .iter()
            .find(|rg| rg.id() == id)
            .unwrap()
    }

    pub fn get_mgs(&self) -> Option<&Resource> {
        self.lustre_resources()
            .find(|res| res.parameters.get("kind").unwrap() == "mgs")
    }

    pub fn hosts(&self) -> impl Iterator<Item = &Arc<Host>> {
        self.hosts.values()
    }

    pub fn get_host(&self, name: &str) -> Option<&Arc<Host>> {
        self.hosts.get(name)
    }

    /// Create a Cluster given a path to a config file.
    pub fn from_config(config: Option<String>) -> HandledResult<Self> {
        let args = crate::manager::Cli {
            config,
            ..Default::default()
        };
        Self::new(args)
    }

    /// Create a Cluster given a context. The context contains the arguments, which holds the
    /// (optional) path to the config file.
    pub fn new(args: manager::Cli) -> HandledResult<Self> {
        let configpath = match &args.config {
            Some(path) => path,
            None => &crate::default_config_path(),
        };
        let config = std::fs::read_to_string(configpath).handle_err(|e| {
            eprintln!("Could not open config file \"{configpath}\": {e}");
        })?;

        let config: crate::config::Config = serde_yaml::from_str(&config).handle_err(|e| {
            eprintln!("Could not parse config file \"{configpath}\": {e}");
        })?;

        let state = match &args.statefile {
            Some(f) => Some(State::new(f)?),
            None => None,
        };

        let mut new = Cluster {
            resource_groups: Vec::new(),
            hosts: HashMap::new(),
            num_zpools: 0,
            num_targets: 0,
            args: args.clone(),
            failover: false,
            state,
        };

        let hosts: HashMap<String, Arc<Host>> = config
            .hosts
            .iter()
            .map(|host| (host.hostname.clone(), Arc::new(Host::from_config(host))))
            .collect();

        for config_host in config.hosts.into_iter() {
            let host = hosts.get(&config_host.hostname).ok_or(()).handle_err(|_| {
                eprintln!(
                    "failed to find host '{}' in cluster config",
                    config_host.hostname
                );
            })?;
            let host = Arc::clone(host);

            let (failover_hostname, failover_host): (&str, Option<Arc<Host>>) =
                match &config.failover_pairs {
                    Some(pairs) => {
                        new.failover = true;
                        let failover_hostname = get_failover_partner(pairs, &config_host.hostname)
                            .ok_or(())
                            .handle_err(|_| {
                                eprintln!(
                                "failed to find failover partner for host '{}' in cluster config",
                                config_host.hostname
                            );
                            })?;
                        let failover_host =
                            hosts.get(failover_hostname).ok_or(()).handle_err(|_| {
                                eprintln!(
                                    "failed to find failover host '{}' in cluster config",
                                    failover_hostname
                                );
                            })?;
                        (failover_hostname, Some(Arc::clone(failover_host)))
                    }
                    None => ("<none>", None),
                };
            // NOTE: .clone() needed dur to later use by 'rg', which may not need 'failover_host'
            // in the future.
            host.set_failover_partner(failover_host.clone())
                .handle_err(|_| {
                    eprintln!(
                        "failed to set failover partner '{}' for host '{}'",
                        failover_hostname, config_host.hostname
                    )
                })?;

            let mut rg =
                Self::one_host_resource_groups(config_host, host, failover_host, args.clone());
            new.resource_groups.append(&mut rg);
        }

        // In the Cluster object, hosts should be mapped by their "unique" ID, which is different
        // in the test environment and a "real" environment. The id() method on host gives the
        // right value:
        let hosts = hosts.into_values().map(|host| (host.id(), host)).collect();

        new.hosts = hosts;

        Ok(new)
    }

    /// Given a config::Host object, convert it into a vector of ResourceGroups where each
    /// ResourceGroup represents a complete dependency tree of resources on the Host.
    fn one_host_resource_groups(
        config_host: crate::config::Host,
        host: Arc<Host>,
        failover_host: Option<Arc<Host>>,
        args: manager::Cli,
    ) -> Vec<ResourceGroup> {
        use std::cell::RefCell;
        use std::rc::Rc;

        /// This type exists for convenience while building the resouce dependency tree.
        /// A TransitionalResource knows both its parent (via me.requires),
        /// and (some of) its children.
        #[derive(Debug, Clone)]
        struct TransitionalResource {
            me: crate::config::Resource,
            children: RefCell<Vec<Rc<TransitionalResource>>>,
            id: String,
        }

        impl TransitionalResource {
            /// Given a TransitionalResource, recursively converts it into a Resource.
            ///
            /// This method assumes that self is the sole owner of self.children, meaning that it
            /// holds the sole reference to those children. All other references must have been
            /// dropped. This will panic if there are outstanding references!
            fn into_resource(
                self,
                host: Arc<Host>,
                failover_host: Option<Arc<Host>>,
                args: manager::Cli,
            ) -> Resource {
                let dependents = RefCell::into_inner(self.children)
                    .into_iter()
                    .map(|child| {
                        Rc::into_inner(child).unwrap().into_resource(
                            Arc::clone(&host),
                            failover_host.clone(),
                            args.clone(),
                        )
                    })
                    .collect();
                Resource::from_config(self.me, dependents, host, failover_host, self.id, args)
            }
        }

        let resources: HashMap<String, TransitionalResource> = config_host
            .resources
            .into_iter()
            .map(|(id, res)| {
                let trans_res = TransitionalResource {
                    me: res,
                    children: RefCell::new(Vec::new()),
                    id: id.clone(),
                };
                (id, trans_res)
            })
            .collect();

        // This will hold the roots of the resource dependency trees:
        let mut roots: Vec<Rc<TransitionalResource>> = Vec::new();
        // While building the dependency trees, it will be necessary to look up a resource in its
        // tree given its ID, so processed_nodes enables that. It uses Rc<> to share a reference to
        // the same underlying resources as roots.
        let mut processed_nodes: HashMap<String, Rc<TransitionalResource>> = HashMap::new();

        for (id, res) in resources.iter() {
            let this_resource = Rc::new(res.clone());
            processed_nodes.insert(id.clone(), Rc::clone(&this_resource));
            match &this_resource.me.requires {
                Some(parent) => {
                    // Depending on whether this_resource's parent appeared before or after this
                    // resource in the iteration order, we need to get a reference to it from
                    // either processed_nodes, or resources.
                    let parent = match processed_nodes.get(parent) {
                        Some(parent) => parent,
                        // TODO: rather than unwrap here, return an error so that the program can
                        // report to the user that the config was invalid.
                        None => resources.get(parent).unwrap(),
                    };
                    parent.children.borrow_mut().push(this_resource);
                }
                None => {
                    // This resource is a root, so add to root list:
                    roots.push(this_resource);
                }
            };
        }

        // Drop all non-root references to the TransitionalResources so that the returned vector
        // can take ownership of them with into_inner():
        std::mem::drop(processed_nodes);
        std::mem::drop(resources);

        roots
            .into_iter()
            .map(|root| {
                let root = Rc::into_inner(root).unwrap().into_resource(
                    Arc::clone(&host),
                    failover_host.clone(),
                    args.clone(),
                );
                ResourceGroup::new(root, args.clone())
            })
            .collect()
    }

    /// Write a Record entry into the Cluster's statefile.
    pub fn write_record(&self, record: Record) -> HandledResult<()> {
        // TODO: The failure of this method should probably be handled in some intelligent way.
        self.state.as_ref().unwrap().write_record(record)
    }

    pub async fn write_record_nonblocking(self: Arc<Self>, record: Record) -> HandledResult<()> {
        tokio::task::spawn_blocking(move || {
            let cluster = &self;
            cluster.write_record(record)
        })
        .await
        .unwrap()
    }

    /// Print out a summary of the cluster to stdout. Mainly intended for debugging purposes.
    pub fn print_summary(&self) {
        println!("=== Resource Groups ===");
        for rg in &self.resource_groups {
            for res in rg.resources() {
                print!("{}: ", res.id);
                println!("{}", res.params_string());
                println!("\thome node: {}", res.home_node.id());
                println!(
                    "\tfailover node: {:?}",
                    res.failover_node.as_ref().map(|h| h.id())
                );
            }
        }

        println!();
        println!("=== Hosts ===");
        for host in self.hosts.values() {
            println!("{}", host);
            println!("\tfence agent: {:?}", host.fence_agent());
        }
    }
}

/// Given a list `pairs` of failover pairs, and a hostname `name`, return its partner, if one
/// exists.
pub fn get_failover_partner<'pairs>(
    pairs: &'pairs [Vec<String>],
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
