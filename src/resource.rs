// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
};

use {futures::future, tokio::sync::Notify};

use crate::{halo_capnp::*, host::*, manager::MgrContext, remote::ocf};

/// Resource Group contains a zpool resource together with all of the Lustre resources that depend
/// on it.
#[derive(Debug)]
pub struct ResourceGroup {
    pub root: Resource,
    overall_status: Mutex<ResourceStatus>,

    /// A notification mechanism used to tell a resource group management task to cancel. This
    /// could occur, for example, because the host that the task is using is going to be fenced.
    pub cancel: tokio::sync::Notify,

    /// A notification mechanism to tell a resource group management task to stop so that
    /// management can be handed over tot he partner host.
    // TODO: rather than using a separate Notify object, it might be better to switch to using a
    // single object that can carry data (i.e., an Enum that says the reason for the notification)
    pub switch_host: tokio::sync::Notify,
}

impl ResourceGroup {
    pub fn new(root: Resource) -> Self {
        assert!(root.kind == "heartbeat/ZFS");
        Self {
            root,
            overall_status: Mutex::new(ResourceStatus::Unknown),
            cancel: Notify::new(),
            switch_host: Notify::new(),
        }
    }

    pub fn id(&self) -> &str {
        &self.root.id
    }

    pub fn home_node(&self) -> &Arc<Host> {
        &self.root.home_node
    }

    /// The host-driven resource management loop manages resources on a given location until an
    /// error is observed, at which point it returns back to the host management code so that the
    /// host can take the appropriate action, whether that be fencing or trying again.
    pub async fn manage_loop(
        &self,
        client: &ocf_resource_agent::Client,
        loc: Location,
    ) -> capnp::Error {
        println!("managing rg {}", self.id());
        let body = async || -> Result<(), capnp::Error> {
            loop {
                self.update_resources(client, loc).await?;
                match self.get_overall_status() {
                    ResourceStatus::Stopped => self.try_start_resources(client, loc).await,
                    ResourceStatus::RunningOnHome | ResourceStatus::RunningOnAway => {
                        self.update_resources(client, loc).await?
                    }
                    other => {
                        eprintln!("Handle resource status {other:?}");
                        todo!()
                    }
                };
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        };

        body().await.unwrap_err()
    }

    pub async fn observe_loop(&self, client: &ocf_resource_agent::Client) -> capnp::Error {
        let body = async || -> Result<(), capnp::Error> {
            loop {
                self.update_resources(client, Location::Home).await?;
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        };

        body().await.unwrap_err()
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
    async fn update_resources(
        &self,
        client: &ocf_resource_agent::Client,
        loc: Location,
    ) -> Result<(), capnp::Error> {
        let futures = self
            .resources()
            .map(|r| async move { (r, r.monitor_client(client).await) });

        let statuses = future::join_all(futures).await;

        let mut res = Ok(());
        for (resource, status) in statuses.into_iter() {
            match status {
                Ok(AgentReply::Success(ocf::Status::Success)) => {
                    resource.set_status(if loc == Location::Home {
                        ResourceStatus::RunningOnHome
                    } else {
                        ResourceStatus::RunningOnAway
                    })
                }
                Ok(AgentReply::Success(ocf::Status::Error(error_type, message))) => {
                    eprintln!("Remote agent returned error: {error_type:?}: {message}");
                    // TODO: create an ResourceStatus::Error for this case?
                    resource.set_status(ResourceStatus::Stopped)
                }
                Ok(AgentReply::Error(message)) => {
                    eprintln!("Remote agent could not run resource program: {message}");
                    resource.set_status(ResourceStatus::Stopped)
                }
                Err(e) => {
                    resource.set_status(ResourceStatus::Unknown);
                    res = Err(e);
                }
            }
        }
        self.update_overall_status();
        res
    }

    /// Attempt to start the resources in this resource group on the given location.
    // TODO: this function should probably return Result<(), capnp::Error> when there is an RPC
    // error, rather than doing nothing - in order to be consistent with the error model of the
    // other resource management routines. Currently, if there is an RPC error in this routine, it
    // won't be noticed until the next iteration of the main loop that next checks the resource
    // status.
    async fn try_start_resources(&self, client: &ocf_resource_agent::Client, loc: Location) {
        self.root.start_if_needed_recursive(client, loc).await;
    }

    pub async fn stop_resources(
        &self,
        client: &ocf_resource_agent::Client,
    ) -> Result<AgentReply, capnp::Error> {
        self.root.stop_recursive(client).await
    }

    fn get_overall_status(&self) -> ResourceStatus {
        self.overall_status.lock().unwrap().clone()
    }

    fn set_overall_status(&self, new_status: ResourceStatus) {
        *self.overall_status.lock().unwrap() = new_status;
    }

    /// Update the ResourceGroup's overall status based on the collected statuses of its members.
    ///
    /// The overall status becomes the "worst" status of any member. For example, if most members
    /// are started but one member is stopped, the overall status is stopped.
    fn update_overall_status(&self) {
        let statuses = self.resources().map(|r| r.get_status());

        let overall_status = ResourceStatus::get_worst(statuses.into_iter());

        self.set_overall_status(overall_status);
    }

    pub fn resources(&self) -> ResourceIterator<'_> {
        ResourceIterator {
            queue: VecDeque::from([&self.root]),
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
    ///     [("mountpoint", "/mnt/ost1"), ("target", "ost1")]
    pub parameters: HashMap<String, String>,

    /// The resources which depend on this resource.
    /// For example, Lustre targets depend on their containing zpool, so the Zpool resource's
    /// dependents would be the Lustre resources that it hosts.
    pub dependents: Vec<Resource>,

    /// Unique identifier for the resource.
    pub id: String,

    pub managed: Mutex<bool>,

    // TODO: better privacy here
    pub status: Mutex<ResourceStatus>,
    pub home_node: Arc<Host>,
    pub failover_node: Option<Arc<Host>>,

    pub context: Arc<MgrContext>,
}

impl Resource {
    pub fn from_config(
        res: crate::config::Resource,
        dependents: Vec<Resource>,
        home_node: Arc<Host>,
        failover_node: Option<Arc<Host>>,
        context: Arc<MgrContext>,
        id: String,
    ) -> Self {
        Resource {
            kind: res.kind,
            parameters: res.parameters,
            dependents,
            status: Mutex::new(ResourceStatus::Unknown),
            home_node,
            failover_node,
            context,
            id,
            managed: Mutex::new(true),
        }
    }

    /// Recursively start a resource as well as all of its dependents.
    /// Updates the status of each resource based on the outcome of the start attempt.
    async fn start_if_needed_recursive(&self, client: &ocf_resource_agent::Client, loc: Location) {
        // If this resource is already running, don't bother doing anything:
        if !self.is_running() {
            match self.start_client(client).await {
                Ok(AgentReply::Success(ocf::Status::Success)) => self.set_running_on_loc(loc),
                Ok(AgentReply::Success(_)) => self.set_status(ResourceStatus::Stopped),
                Ok(AgentReply::Error(e)) => {
                    eprintln!("Warning: Remote agent returned error {e} when attempting to start resource {}.",
                        self.id);
                    self.set_status(ResourceStatus::Unknown);
                }
                e => {
                    eprintln!(
                        "Error: '{e:?}' when attempting to start resource '{}'.",
                        self.id
                    );
                    self.set_status(ResourceStatus::Unknown);
                }
            };
        }

        // Only start the dependents of this resource if it actually started succesfully:
        if self.is_running() {
            let futures = self
                .dependents
                .iter()
                .map(|r| r.start_if_needed_recursive(client, loc));
            future::join_all(futures).await;
        }
    }

    async fn stop_recursive(
        &self,
        client: &ocf_resource_agent::Client,
    ) -> Result<AgentReply, capnp::Error> {
        let results = self.dependents.iter().map(|r| r.stop_recursive(client));

        if let Some(err) = future::join_all(results)
            .await
            .into_iter()
            .find(|res| res.is_err())
        {
            return err;
        }

        self.stop_client(client).await
    }

    /// Perform a monitor RPC for this resource given a client.
    pub async fn monitor_client(
        &self,
        client: &ocf_resource_agent::Client,
    ) -> Result<AgentReply, capnp::Error> {
        remote_ocf_operation_given_client(self, client, ocf_resource_agent::Operation::Monitor)
            .await
    }

    /// Perform a start RPC for this resource given a client.
    pub async fn start_client(
        &self,
        client: &ocf_resource_agent::Client,
    ) -> Result<AgentReply, capnp::Error> {
        remote_ocf_operation_given_client(self, client, ocf_resource_agent::Operation::Start).await
    }

    /// Perform a stop RPC for this resource given a client.
    pub async fn stop_client(
        &self,
        client: &ocf_resource_agent::Client,
    ) -> Result<AgentReply, capnp::Error> {
        remote_ocf_operation_given_client(self, client, ocf_resource_agent::Operation::Stop).await
    }

    /// Perform a monitor RPC for this resource.
    pub async fn monitor(&self, loc: Location) -> Result<AgentReply, AgentError> {
        tokio::task::LocalSet::new()
            .run_until(async {
                remote_ocf_operation(self, loc, ocf_resource_agent::Operation::Monitor).await
            })
            .await
    }

    /// Perform a start RPC for this resource.
    pub async fn start(&self, loc: Location) -> Result<AgentReply, AgentError> {
        tokio::task::LocalSet::new()
            .run_until(async {
                remote_ocf_operation(self, loc, ocf_resource_agent::Operation::Start).await
            })
            .await
    }

    /// Perform a stop RPC for this resource.
    pub async fn stop(&self) -> Result<AgentReply, AgentError> {
        tokio::task::LocalSet::new()
            .run_until(async {
                remote_ocf_operation(self, Location::Home, ocf_resource_agent::Operation::Stop)
                    .await
            })
            .await
    }

    pub fn get_status(&self) -> ResourceStatus {
        self.status.lock().unwrap().clone()
    }

    pub fn set_status(&self, status: ResourceStatus) {
        let mut old_status = self.status.lock().unwrap();
        let old_status_copy = old_status.clone();
        *old_status = status.clone();
        std::mem::drop(old_status);
        if self.context.args.verbose && old_status_copy != status {
            let _ = self.context.out_stream.writeln(
                self.status_update_string(old_status_copy, status)
                    .as_bytes(),
            );
        }
    }

    pub fn status_update_string(&self, old: ResourceStatus, new: ResourceStatus) -> String {
        format!(
            "Updating status of resource {} from {:?} to {:?}",
            self.params_string(),
            old,
            new,
        )
    }

    fn is_running(&self) -> bool {
        matches!(
            self.get_status(),
            ResourceStatus::RunningOnHome | ResourceStatus::RunningOnAway
        )
    }

    pub fn set_running_on_loc(&self, loc: Location) {
        match loc {
            Location::Home => self.set_status(ResourceStatus::RunningOnHome),
            Location::Away => self.set_status(ResourceStatus::RunningOnAway),
        };
    }

    /// Return a string representation of this resource's parameters in a predictable way.
    pub fn params_string(&self) -> String {
        let mut params: Vec<(&String, &String)> = self.parameters.iter().collect();
        params.sort();
        let mut output: String = String::from("{");
        params.iter().enumerate().for_each(|(i, (k, v))| {
            if i == params.len() - 1 {
                output.push_str(&format!("\"{k}\": \"{v}\"}}"));
            } else {
                output.push_str(&format!("\"{k}\": \"{v}\", "));
            }
        });
        output
    }

    /// Get management status of resource, to be used in status
    pub fn get_managed(&self) -> bool {
        let managed_status = self.managed.lock().unwrap();
        *managed_status
    }

    /// Sets resources's managed status to true, used recursively for dependents.
    pub fn set_managed(&self, managed: bool) {
        let mut managed_status = self.managed.lock().unwrap();
        *managed_status = managed;
        for d in &self.dependents {
            d.set_managed(managed);
        }
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
    /// service and the remote agent.
    Unknown,

    /// An operation failed in a way that the manager cannot address, and so the resource is
    /// impossible to manage. For example, if a "start" operation is performed, and fails, there
    /// is nothing the manager can do because there is most likely a configuration error or similar
    /// state that requires admin intervention.
    Error(String),

    /// The resource is not running anywhere.
    Stopped,

    /// The resource is running on its failover node.
    RunningOnAway,

    /// The resource is running on its home node.
    RunningOnHome,
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
        list.min().unwrap_or(Self::Unknown)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Location {
    Home,
    Away,
}

#[cfg(test)]
mod tests {
    use super::ResourceStatus;

    #[test]
    fn test_get_worst() {
        assert_eq!(
            ResourceStatus::Unknown,
            ResourceStatus::get_worst(
                vec![ResourceStatus::Unknown, ResourceStatus::Error("".to_string())].into_iter()
            )
        );

        assert_eq!(
            ResourceStatus::get_worst(vec![].into_iter()),
            ResourceStatus::Unknown
        );

        assert_eq!(
            ResourceStatus::get_worst(
                vec![ResourceStatus::RunningOnHome, ResourceStatus::RunningOnAway].into_iter()
            ),
            ResourceStatus::RunningOnAway,
        );
    }
}
