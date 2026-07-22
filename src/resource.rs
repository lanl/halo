// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
};

use {
    futures::future,
    log::{error, warn},
};

use crate::{halo_capnp::*, host::*, manager, remote::ocf};

#[derive(Debug)]
pub enum ManagementError {
    /// An error that occured due an action failing unexpectedly, typically indicating a
    /// configuration issue or other problem requiring admin intervention.
    Configuration,

    /// An error that occurred due to network connection failing.
    Connection,
}

impl From<capnp::Error> for ManagementError {
    fn from(e: capnp::Error) -> Self {
        match e.kind {
            capnp::ErrorKind::Disconnected => ManagementError::Connection,
            _ => ManagementError::Configuration,
        }
    }
}

// Given multiple tasks that each could have produced an error, it is helpful to extract the
// "worst" error, if any error ocurred.
//
// ManagementError::Configuration typically indicates an issue that requires admin intervention, so if
// such an error is present it is immediately returned.
//
// A ManagementError::Connection is "less severe" in the sense that the management system can automically
// handle it by fencing the Host involved. It is returned only if no Configuration errors are present.
fn get_worst_error(
    results: impl Iterator<Item = Result<(), ManagementError>>,
) -> Result<(), ManagementError> {
    let mut res = Ok(());

    for result in results {
        match result {
            Ok(()) => {}
            Err(ManagementError::Configuration) => return result,
            Err(ManagementError::Connection) => res = result,
        }
    }

    res
}

/// Resource Group contains a zpool resource together with all of the Lustre resources that depend
/// on it.
#[derive(Debug)]
pub struct ResourceGroup {
    pub root: Resource,
    managed: Mutex<bool>,
    args: manager::Cli,
    home_node: HostKnowledge,
    failover_node: Option<HostKnowledge>,
}

#[derive(Debug)]
struct HostKnowledge {
    host: Arc<Host>,
    state: Mutex<State>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    Running,
    Stopped,
    Unknown,
}

impl HostKnowledge {
    fn new(host: Arc<Host>) -> Self {
        Self {
            host,
            state: Mutex::new(State::Unknown),
        }
    }

    fn state(&self) -> State {
        *self.state.lock().unwrap()
    }

    fn set_state(&self, state: State) {
        *self.state.lock().unwrap() = state;
    }
}

impl ResourceGroup {
    pub fn new(
        root: Resource,
        args: manager::Cli,
        home_node: Arc<Host>,
        failover_node: Option<Arc<Host>>,
    ) -> Self {
        Self {
            root,
            managed: Mutex::new(true),
            args,
            home_node: HostKnowledge::new(home_node),
            failover_node: failover_node.map(HostKnowledge::new),
        }
    }

    pub fn id(&self) -> &str {
        &self.root.id
    }

    pub fn home_node(&self) -> &Arc<Host> {
        &self.home_node.host
    }

    pub fn failover_node(&self) -> Option<&Arc<Host>> {
        self.failover_node.as_ref().map(|h| &h.host)
    }

    /// The host-driven resource management loop manages resources on a given location until
    /// either:
    ///
    ///   - An error is observed: it returns back to the host management code so that the host can
    ///     take the appropriate action, whether that be fencing or trying again;
    ///
    ///   - The root resource is discovered to be stopped, and the resource group is unmanaged: it
    ///     returns back to the host management code so that the host can begin checing the
    ///     failover partner to see if the resource was started there (manual failover).
    pub async fn manage_loop(&self, client: &Client, loc: Location) -> Result<(), ManagementError> {
        loop {
            self.update_resources(client).await?;
            match self.get_overall_status_on_host(&client.name) {
                ResourceStatus::Stopped => {
                    if self.get_managed() {
                        self.start_resources(client, loc).await?;
                    } else if !self.root.is_running() {
                        return Ok(());
                    }
                }
                ResourceStatus::Running => {}
                other => {
                    warn!("resource status was unexpected: {other:?}");
                    return Err(ManagementError::Configuration);
                }
            };
            tokio::time::sleep(tokio::time::Duration::from_millis(self.args.sleep_time)).await;
        }
    }

    /// Observe some resources.
    ///
    /// Exits either when an error was observed, or if the exit_if_resource_stopped flag is set, it
    /// will exit with Ok(()) if the entire resource group has stopped.
    pub async fn observe_loop(
        &self,
        client: &Client,
        exit_if_resource_stopped: bool,
    ) -> Result<(), ManagementError> {
        loop {
            self.update_resources(client).await?;
            if exit_if_resource_stopped && !self.resources().any(|res| res.is_running()) {
                return Ok(());
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(self.args.sleep_time)).await;
        }
    }

    /// Attempt to start the resources in this resource group on the given location.
    async fn start_resources(&self, client: &Client, loc: Location) -> Result<(), ManagementError> {
        let res = self.root.start_if_needed_recursive(client, loc).await;

        match res {
            Ok(()) => self.set_host_state(State::Running, &client.name),
            Err(ManagementError::Connection) => self.set_host_state(State::Unknown, &client.name),
            _ => {}
        };

        res
    }

    /// Attempt to stop the resources in this resource group.
    pub async fn stop_resources(&self, client: &Client) -> Result<(), ManagementError> {
        let res = self.root.stop_recursive(client).await;

        match res {
            Ok(()) => self.set_host_state(State::Stopped, &client.name),
            Err(ManagementError::Connection) => self.set_host_state(State::Unknown, &client.name),
            _ => {}
        }

        res
    }

    pub fn get_overall_status_on_host(&self, host: &str) -> ResourceStatus {
        let statuses = self.resources().map(|r| r.status_on_host(host));

        ResourceStatus::get_worst(statuses.into_iter())
    }

    pub fn resources(&self) -> ResourceIterator<'_> {
        ResourceIterator {
            queue: VecDeque::from([&self.root]),
        }
    }

    /// Get management status of resource group, to be used in status
    pub fn get_managed(&self) -> bool {
        let managed_status = self.managed.lock().unwrap();
        *managed_status
    }

    /// Sets resources group's managed status
    pub fn set_managed(&self, managed: bool) {
        let mut managed_status = self.managed.lock().unwrap();

        // When a resource transitions from being unmanaged to being managed, any assumptions about
        // its state MUST be revalidated before the manager can take any actions on it. Therefore,
        // clear any out of date knowledge on whether it was known to be running somewhere.
        if !*managed_status && managed {
            *self.home_node.state.lock().unwrap() = State::Unknown;
            if let Some(ref failover_node) = self.failover_node {
                *failover_node.state.lock().unwrap() = State::Unknown;
            }
        }

        *managed_status = managed;
    }

    pub fn has_been_stopped(&self, host: &str) {
        self.set_host_state(State::Stopped, host);
    }

    pub fn is_running_nowhere(&self) -> bool {
        self.home_node.state() == State::Stopped
            && self.failover_node.as_ref().unwrap().state() == State::Stopped
    }

    pub fn is_stopped_at_location(&self, loc: Location) -> bool {
        let state = match loc {
            Location::Home => self.home_node.state(),
            Location::Away => self.failover_node.as_ref().expect("must be set").state(),
        };

        state == State::Stopped
    }

    fn set_host_state(&self, state: State, host_id: &str) {
        let host = if host_id == self.home_node.host.id() {
            &self.home_node
        } else {
            let failover_host = self
                .failover_node
                .as_ref()
                .expect("Failover node must be set.");
            if host_id != failover_host.host.id() {
                panic!(
                    "Unexpeced host id: {host_id} for setting resource state of {}",
                    self.id()
                );
            }
            failover_host
        };

        host.set_state(state);

        self.assert_knowledge_invariant();
    }

    fn assert_knowledge_invariant(&self) {
        if let Some(ref failover_node) = self.failover_node {
            let home_state = self.home_node.state();
            let failover_state = failover_node.state();
            if home_state == State::Running && failover_state == State::Running {
                panic!("Resource group {} violated invariant: system believes it to be running on both nodes.",
                    self.id());
            }
        }
    }

    fn assert_not_running_elsewhere(&self, host: &str) {
        if !self.failover_node.is_some() {
            // This assertion is irrelevant for non-HA clusters, so just return.
            return;
        };

        for (other_host, status) in self.root.status() {
            if other_host == host {
                continue;
            }

            if status == ResourceStatus::Running {
                panic!("Manager tried to manage resource {} on host {} while it still believes it to be running on host {}.", self.id(), host, other_host);
            }
        }
    }

    /// Check the statuses of each of the resources in this resource group.
    ///
    /// This function updates the status of each resource (zpool and target) in the resource
    /// group, and the host.
    ///
    /// When checking a resource's status, errors can occur at multiple levels:
    ///
    /// - Errors in communicating with the remote agent--for example, "Connection timed out"--lead
    ///   to this function returning an Err() variant containing the error. Nothing can be
    ///   concluded about the status of a remote resource in such a case, so the status is set to
    ///   Unknown.
    /// - When communication with the remote agent succesfully occurred, but the remote agent
    ///   returned an error status, this function returns an Ok() variant and sets the resource
    ///   status to the appropriate value to indicate the kind of error returned.
    pub async fn update_resources(&self, client: &Client) -> Result<bool, ManagementError> {
        self.assert_not_running_elsewhere(&client.name);

        let futures = self.resources().map(|r| r.update_status(client));

        get_worst_error(future::join_all(futures).await.into_iter()).inspect_err(|e| {
            if matches!(e, ManagementError::Connection) {
                self.set_host_state(State::Unknown, &client.name);
                self.root.set_status_recursive(
                    ResourceStatus::Unknown("Connection to remote host lost.".to_string()),
                    &client.name,
                );
            }
        })?;

        if self.root.is_running_on_loc(&client.name) {
            self.set_host_state(State::Running, &client.name);
            Ok(true)
        } else {
            self.set_host_state(State::Stopped, &client.name);
            Ok(false)
        }
    }
}

/// This iterator visits all of the Resources in a dependency tree in breadth-first order.
pub struct ResourceIterator<'a> {
    queue: VecDeque<&'a Resource>,
}

impl<'a> Iterator for ResourceIterator<'a> {
    type Item = &'a Resource;

    fn next(&mut self) -> Option<Self::Item> {
        let res = self.queue.pop_front()?;

        self.queue
            .append(&mut VecDeque::from_iter(res.dependents.iter()));

        Some(res)
    }
}

#[derive(Debug)]
pub struct Resource {
    /// The kind of the resource, i.e., Lustre target, zpool, etc. This should be in the form of an
    /// OCF resource agent identifier, e.g.:
    ///   - "lustre/Lustre"
    ///   - "heartbeat/ZFS"
    pub kind: String,

    /// The parameters of the resource as key-value pairs. For example, for Lustre, this would
    /// be something like:
    ///     [("mountpoint": "/mnt/ost1"), ("target": "ost1")]
    pub parameters: HashMap<String, String>,

    /// The resources which depend on this resource.
    /// For example, Lustre targets depend on their containing zpool, so the Zpool resource's
    /// dependents would be the Lustre resources that it hosts.
    dependents: Vec<Resource>,

    /// Unique identifier for the resource.
    pub id: String,

    /// How many instances of this resource are permitted to run simultaneously. Normally a
    /// resource has the requirement that it cannot run on multiple hosts at the same time. For
    /// such "mutually exclusive" resources, count will equal 1. When count is N, the resource can
    /// run on up to N hosts simultanesouly.
    _count: usize,

    pub state: ResourceState,

    pub args: manager::Cli,
}

pub type HostStatusMap = HashMap<String, ResourceStatus>;

#[derive(Debug, Clone)]
pub struct ResourceState {
    inner: Arc<Mutex<HostStatusMap>>,
}

impl ResourceState {
    pub fn new(host_list: Vec<String>) -> Self {
        let inner: HostStatusMap = host_list
            .into_iter()
            .map(|h| {
                (
                    h,
                    ResourceStatus::Unknown("Manager is starting up".to_string()),
                )
            })
            .collect();

        Self {
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    pub fn add_hosts(&self, hosts: &[String]) {
        for host in hosts {
            self.inner.lock().unwrap().insert(
                host.to_string(),
                ResourceStatus::Unknown("Manager is starting up".to_string()),
            );
        }
    }

    fn get(&self) -> HostStatusMap {
        self.inner.lock().unwrap().clone()
    }

    /// Update the status for `host` to `new_status`.
    /// Returns the old status.
    fn update(&self, new_status: ResourceStatus, host: &str) -> ResourceStatus {
        let mut map = self.inner.lock().unwrap();
        let ent = map
            .get_mut(host)
            .expect("Host {host} must have an entry in the status map");
        let old_status = ent.clone();
        *ent = new_status;
        old_status
    }
}

impl Resource {
    pub fn from_config2(
        res: &crate::config::Resource2,
        config: &crate::config::Config2,
        status_list: &HashMap<String, (usize, ResourceState)>,
        args: manager::Cli,
    ) -> Self {
        let dependents = res
            .dependents
            .iter()
            .map(|r| {
                let r = config.get_resource(r);
                Self::from_config2(r, config, status_list, args.clone())
            })
            .collect();

        let (count, state) = status_list.get(&res.name).unwrap();

        Self {
            kind: res.kind.clone(),
            parameters: res.parameters.clone(),
            dependents,
            _count: *count,
            state: state.clone(),
            id: res.name.clone(),
            args,
        }
    }

    /// This method checks if the resource is running on the system connected via the given Client.
    pub async fn update_status(&self, client: &Client) -> Result<(), ManagementError> {
        match self.monitor(client).await {
            Ok(AgentReply::Success(ocf::Status::Success)) => {
                self.set_running_on_loc(&client.name);
                Ok(())
            }
            Ok(AgentReply::Success(ocf::Status::Error(kind, reason))) => match kind {
                ocf::OcfError::ErrNotRunning => {
                    self.set_status(ResourceStatus::Stopped, &client.name);
                    Ok(())
                }
                _ => {
                    self.set_status(ResourceStatus::Error(reason), &client.name);
                    Err(ManagementError::Configuration)
                }
            },
            Ok(AgentReply::Error(reason)) => {
                self.set_status(ResourceStatus::Error(reason), &client.name);
                Err(ManagementError::Configuration)
            }
            Err(e) => {
                self.set_status(ResourceStatus::Unknown(format!("{e}")), &client.name);
                Err(e.into())
            }
        }
    }

    /// Recursively start a resource as well as all of its dependents.
    /// Updates the status of each resource based on the outcome of the start attempt.
    async fn start_if_needed_recursive(
        &self,
        client: &Client,
        loc: Location,
    ) -> Result<(), ManagementError> {
        // If this resource is already running, don't bother doing anything:
        if !self.is_running_on_loc(&client.name) {
            warn!(
                "Attempting to start resource {} on {}.",
                self.id,
                match loc {
                    Location::Home => "its home node",
                    Location::Away => "its failover node",
                }
            );
            match self.start(client).await {
                // Agent replies that the resource was started succesfully.
                Ok(AgentReply::Success(ocf::Status::Success)) => {
                    self.set_running_on_loc(&client.name)
                }
                // Agent replies that it could not start the resource. This is likely due to a
                // misconfiguration or other issue that requires admin intervention, so return an
                // error.
                Ok(AgentReply::Success(ocf::Status::Error(_, reason))) => {
                    self.set_status(ResourceStatus::Error(reason), &client.name);
                    return Err(ManagementError::Configuration);
                }
                // Agent replies that it could not run the resource management script. This is
                // likely due to a misconfiguration like the script not being installed, so return
                // an error.
                Ok(AgentReply::Error(reason)) => {
                    error!("Warning: Remote agent returned error {reason} when attempting to start resource {}.",
                        self.id);
                    self.set_status(ResourceStatus::Error(reason), &client.name);
                    return Err(ManagementError::Configuration);
                }
                // An RPC error occurred, for example, because the connection timed out or was
                // reset. Management cannot proceed in a such a case, so return an error.
                Err(e) => {
                    warn!(
                        "Error: '{e:?}' when attempting to start resource '{}'.",
                        self.id
                    );
                    self.set_status(ResourceStatus::Unknown(format!("{e}")), &client.name);
                    return Err(e.into());
                }
            };
        }

        // Only start the dependents of this resource if it actually started succesfully:
        let futures = self
            .dependents
            .iter()
            .map(|r| r.start_if_needed_recursive(client, loc));

        get_worst_error(future::join_all(futures).await.into_iter())
    }

    async fn stop_recursive(&self, client: &Client) -> Result<(), ManagementError> {
        let results = self.dependents.iter().map(|r| r.stop_recursive(client));

        get_worst_error(future::join_all(results).await.into_iter())?;

        match self.stop(client).await {
            Ok(AgentReply::Success(ocf::Status::Success)) => {
                self.set_status(ResourceStatus::Stopped, &client.name);
                Ok(())
            }
            // Agent replies that it could not stop the resource. This is likely due to a
            // misconfiguration or other issue that requires admin intervention, so return an
            // error.
            Ok(AgentReply::Success(ocf::Status::Error(_, reason))) => {
                self.set_status(ResourceStatus::Error(reason), &client.name);
                Err(ManagementError::Configuration)
            }
            // Agent replies that it could not run the resource management script. This is
            // likely due to a misconfiguration like the script not being installed, so return
            // an error.
            Ok(AgentReply::Error(reason)) => {
                error!("Warning: Remote agent returned error {reason} when attempting to stop resource {}.",
                    self.id);
                self.set_status(ResourceStatus::Error(reason), &client.name);
                Err(ManagementError::Configuration)
            }
            // An RPC error occurred, for example, because the connection timed out or was
            // reset. Management cannot proceed in a such a case, so return an error.
            Err(e) => {
                error!(
                    "Error: '{e:?}' when attempting to stop resource '{}'.",
                    self.id
                );
                self.set_status(ResourceStatus::Unknown(format!("{e}")), &client.name);
                Err(e.into())
            }
        }
    }

    /// Perform a monitor RPC for this resource given a client.
    async fn monitor(&self, client: &Client) -> Result<AgentReply, capnp::Error> {
        remote_ocf_operation_given_client(self, client, ocf_resource_agent::Operation::Monitor)
            .await
    }

    /// Perform a start RPC for this resource given a client.
    async fn start(&self, client: &Client) -> Result<AgentReply, capnp::Error> {
        remote_ocf_operation_given_client(self, client, ocf_resource_agent::Operation::Start).await
    }

    /// Perform a stop RPC for this resource given a client.
    async fn stop(&self, client: &Client) -> Result<AgentReply, capnp::Error> {
        remote_ocf_operation_given_client(self, client, ocf_resource_agent::Operation::Stop).await
    }

    pub fn status(&self) -> HostStatusMap {
        self.state.get()
    }

    pub fn status_on_host(&self, host: &str) -> ResourceStatus {
        self.status()
            .get(host)
            .expect("Host {host} should have a status entry")
            .clone()
    }

    pub fn set_status(&self, new_status: ResourceStatus, host: &str) {
        let old_status = self.state.update(new_status.clone(), host);

        if old_status != new_status {
            warn!(
                "Updating status of resource {} from {:?} to {:?}",
                self.id, old_status, new_status
            )
        }
    }

    fn is_running(&self) -> bool {
        self.state
            .get()
            .values()
            .any(|v| *v == ResourceStatus::Running)
    }

    fn is_running_on_loc(&self, host: &str) -> bool {
        self.status_on_host(host) == ResourceStatus::Running
    }

    pub fn set_running_on_loc(&self, host: &str) {
        self.set_status(ResourceStatus::Running, host);
    }

    /// Return a string representation of this resource's parameters in a predictable way.
    pub fn params_string(&self) -> String {
        let mut params: Vec<(&String, &String)> = self.parameters.iter().collect();
        params.sort();
        let mut output: String = String::from("{");
        params.iter().enumerate().for_each(|(i, (k, v))| {
            if i == params.len() - 1 {
                output.push_str(&format!("\"{k}\": \"{v}\""));
            } else {
                output.push_str(&format!("\"{k}\": \"{v}\", "));
            }
        });
        output.push('}');
        output
    }

    pub fn set_status_recursive(&self, status: ResourceStatus, host: &str) {
        for child in self.dependents.iter() {
            child.set_status_recursive(status.clone(), host);
        }

        self.set_status(status, host);
    }
}

/// The ordering on ResourceStatus is used to rank statuses from "worst" to "best". Statuses that
/// are "worse" should appear first in the enum.
///
/// This ordering is used to determine the "overall" status to assign to a ResourceGroup, when the
/// members of that ResourceGroup may each have their own separate status. If all but one member
/// are RunningOnHome, but one member is Stopped, the group should be considered stopped.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResourceStatus {
    /// The resource's status cannot be determined because communication failed between the manager
    /// service and the remote agent, or because it hasn't been checked yet.
    Unknown(String),

    /// An operation failed in a way that the manager cannot address, and so the resource is
    /// impossible to manage. For example, if a "start" operation is performed, and fails, there
    /// is nothing the manager can do because there is most likely a configuration error or similar
    /// state that requires admin intervention.
    Error(String),

    /// The resource is not running.
    Stopped,

    /// The resource is running.
    Running,
}

impl ResourceStatus {
    /// Given an iterator over ResourceStatuses, determine the "worst" one. This is used to assign
    /// an overall status to a group of resources based on the worst member status.
    ///
    /// If the given iterator is empty, pessimistically assign "Unknown".
    pub fn get_worst<L>(list: L) -> Self
    where
        L: Iterator<Item = ResourceStatus>,
    {
        list.min().unwrap_or(Self::Unknown("".to_string()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Location {
    Home,
    Away,
}

impl Location {
    pub fn swap(self) -> Self {
        match self {
            Location::Home => Location::Away,
            Location::Away => Location::Home,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ResourceStatus;

    #[test]
    fn test_get_worst() {
        assert_eq!(
            ResourceStatus::Unknown("".to_string()),
            ResourceStatus::get_worst(
                vec![
                    ResourceStatus::Unknown("".to_string()),
                    ResourceStatus::Error("".to_string()),
                ]
                .into_iter()
            )
        );

        assert_eq!(
            ResourceStatus::get_worst(vec![].into_iter()),
            ResourceStatus::Unknown("".to_string()),
        );
    }
}
