// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use std::{
    collections::{HashMap, HashSet},
    env,
    sync::Arc,
};

use {futures::future, rustls::pki_types::ServerName, tokio_rustls::TlsConnector};

use crate::{
    config,
    host::*,
    manager,
    resource::*,
    state::{Record, State},
    Handle, HandledResult,
};

/// Cluster is the model used to represent the dynamic state of a cluster in memory.
/// Unlike the persistent model which views a cluster as made up of nodes, which own services,
/// the in-memory model views a cluster as made up of services (storage devices and Lustre
/// targets), and services know which nodes they expect to run on.
///
/// This model is slightly more convenient for performing cluster operations.
pub struct Cluster {
    resource_groups: Vec<ResourceGroup>,

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

    pub tls_args: Option<TlsArgs>,
}

pub struct TlsArgs {
    pub tls_connector: TlsConnector,

    pub domain: ServerName<'static>,
}

impl Cluster {
    /// Apply a state Delta to this Cluster's Hosts.
    pub fn apply_state(&self) {
        let Some(state) = &self.state else {
            return;
        };

        // Track which hosts have been updated here in order to determine hosts that aren't in
        // the cluster.
        let mut unapplied_hosts: HashSet<String> = state.delta.hosts();

        for cluster_host in self.hosts() {
            let cluster_host_id = cluster_host.id();

            if let Some(fenced) = state.delta.hosts_fenced.get(&cluster_host_id) {
                cluster_host.set_fence_attempted(*fenced);
                unapplied_hosts.remove(&cluster_host_id);
            }
            if let Some(active) = state.delta.hosts_activated.get(&cluster_host_id) {
                cluster_host.set_active(*active);
                unapplied_hosts.remove(&cluster_host_id);
            }
        }
        for host in &unapplied_hosts {
            eprintln!("host '{host}' not found in cluster");
        }

        // Track which resources have been updated here in order to determine hosts that aren't
        // in the cluster.
        let mut unapplied_resources: HashSet<String> = state.delta.resources();
        for cluster_rg in self.resource_groups() {
            let cluster_rg_id = cluster_rg.id();

            if let Some(managed) = state.delta.resources_managed.get(cluster_rg_id) {
                cluster_rg.set_managed(*managed);
                unapplied_resources.remove(cluster_rg_id);
            }
        }
        for rg in &unapplied_resources {
            eprintln!("resource '{rg}' not found in cluster");
        }
    }

    pub async fn main_loop(self: Arc<Self>) {
        if self.failover {
            let futures: Vec<_> = self.hosts.values().map(|h| h.manage_ha(&self)).collect();

            let _ = future::join_all(futures).await;
        } else if self.args.manage_resources {
            let futures: Vec<_> = self.hosts.values().map(|h| h.manage(&self)).collect();

            let _ = future::join_all(futures).await;
        } else {
            let futures: Vec<_> = self.hosts.values().map(|h| h.observe(&self)).collect();

            let _ = future::join_all(futures).await;
        }
    }

    pub fn resource_groups(&self) -> impl Iterator<Item = &ResourceGroup> {
        self.resource_groups.iter()
    }

    pub fn resources(&self) -> impl Iterator<Item = &Resource> {
        self.resource_groups
            .iter()
            .flat_map(|group| group.resources())
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

        let tls_args = if args.mtls {
            Some(TlsArgs {
                tls_connector: crate::tls::get_connector()?,
                domain: ServerName::try_from(
                    env::var("HALO_SERVER_DOMAIN_NAME").expect("HALO_SERVER_DOMAIN_NAME not set."),
                )
                .handle_err(|e| eprintln!("Could not create server domain name: {e}"))?,
            })
        } else {
            None
        };

        let mut new = Cluster {
            resource_groups: Vec::new(),
            hosts: HashMap::new(),
            args: args.clone(),
            failover: false,
            state,
            tls_args,
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

        new.apply_state();

        // If the cluster is running in "observe-only" mode, mark every resource group as unmanaged
        if !args.manage_resources {
            for rg in new.resource_groups() {
                rg.set_managed(false);
            }
        }

        Ok(new)
    }

    pub fn from_config2(
        config: &config::Config2,
        args: manager::Cli,
        state: Option<State>,
        tls_args: Option<TlsArgs>,
    ) -> Self {
        let hosts: HashMap<_, _> = config
            .hosts
            .iter()
            .map(|h| (h.hostname.clone(), Arc::new(Host::from_config2(h))))
            .collect();

        // Set failover partners:
        let failover_partners = config.get_failover_partners();
        for host in hosts.values() {
            if let Some(partner) = failover_partners.get(host.raw_name()) {
                let partner = hosts
                    .get(partner)
                    .expect("Host {} referenced but not defined. Invalid config.");

                // TODO: use ? and make this method return HandledResult
                host.set_failover_partner(Some(Arc::clone(partner)))
                    .unwrap();
            } else {
                // TODO: use ? and make this method return HandledResult
                host.set_failover_partner(None).unwrap();
            }
        }

        let mut resource_list: HashMap<String, (usize, ResourceState)> = HashMap::new();

        fn update_resource_list(
            config: &config::Config2,
            list: &mut HashMap<String, (usize, ResourceState)>,
            resource: &config::Resource2,
            hostnames: &[String],
        ) {
            list.entry(resource.name.clone())
                .and_modify(|(count, val)| {
                    *count += 1;
                    val.add_hosts(hostnames);
                })
                .or_insert((1, ResourceState::new(Vec::from(hostnames))));

            for res in &resource.dependents {
                let res = config.get_resource(res);
                update_resource_list(config, list, res, hostnames);
            }
        }

        // First iteration: just determine which resources are shared vs. exclusive.
        for rg in &config.resource_groups {
            let root = config.get_resource(&rg.root);
            let mut hostnames = Vec::new();
            hostnames.push(rg.home_host.clone());
            hostnames.extend_from_slice(&rg.failover_hosts);

            let hostnames: Vec<_> = hostnames
                .iter()
                .map(|h| hosts.get(h).unwrap().id())
                .collect();

            update_resource_list(config, &mut resource_list, root, &hostnames);
        }

        // Second iteration: create the actual resource groups
        let mut resource_groups = Vec::new();
        for config_rg in &config.resource_groups {
            let root = config.get_resource(&config_rg.root);

            let root_resource = Resource::from_config2(root, config, &resource_list, args.clone());

            let home_host = Arc::clone(
                hosts
                    .get(&config_rg.home_host)
                    .expect("Host {h} referenced but not defined. Invalid config."),
            );

            let failover_host = config_rg
                .failover_hosts
                .first()
                .map(|h| {
                    hosts
                        .get(h)
                        .expect("Host {h} referenced but not defined. Invalid config.")
                })
                .map(Arc::clone);

            let rg: ResourceGroup =
                ResourceGroup::new(root_resource, args.clone(), home_host, failover_host);
            resource_groups.push(rg);
        }

        // In the Cluster object, hosts should be mapped by their "unique" ID, which is different
        // in the test environment and a "real" environment. The id() method on host gives the
        // right value:
        let hosts = hosts.into_values().map(|host| (host.id(), host)).collect();

        Self {
            resource_groups,
            hosts,
            args,
            failover: true,
            state,
            tls_args,
        }
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
                let mut host_list = vec![host.id().clone()];
                if let Some(failover_host) = failover_host {
                    host_list.push(failover_host.id().clone());
                };
                Resource::from_config(self.me, dependents, self.id, args, host_list)
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
                ResourceGroup::new(root, args.clone(), Arc::clone(&host), failover_host.clone())
            })
            .collect()
    }

    /// Write a Record entry into the Cluster's statefile.
    pub fn write_record(&self, record: Record) -> HandledResult<()> {
        self.state.as_ref().unwrap().write_record(record)
    }

    pub async fn write_record_nonblocking(self: &Arc<Self>, record: Record) -> HandledResult<()> {
        let cluster = Arc::clone(self);
        tokio::task::spawn_blocking(move || cluster.write_record(record))
            .await
            .unwrap()
    }

    /// Print out a summary of the cluster to stdout. Mainly intended for debugging purposes.
    pub fn print_summary(&self) {
        println!("=== Resource Groups ===");
        for rg in &self.resource_groups {
            println!("\thome node: {}", rg.home_node().id());
            println!("\tfailover node: {:?}", rg.failover_node().map(|h| h.id()));
            for res in rg.resources() {
                print!("{}: ", res.id);
                print!("{}", res.params_string());
                println!(" status_map: {:?}", res.state);
            }
        }

        println!();
        println!("=== Hosts ===");
        for host in self.hosts.values() {
            println!("{}", host);
            println!("\tfence agent: {:?}", host.fence_agent());
        }
    }

    /// Gather our cluster events
    pub fn get_cluster_events(&self) -> Vec<Record> {
        let Some(ref state) = self.state else {
            return Vec::new();
        };
        state.records.lock().unwrap().clone()
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
