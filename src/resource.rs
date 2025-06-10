// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use futures::future;
use std::collections::{HashMap, VecDeque};
use std::error::Error;
use std::sync::{Arc, Mutex};

use crate::halo_capnp::{do_ocf_request, ocf_resource_agent};
use crate::host::*;
use crate::manager::MgrContext;
use crate::remote::ocf;

/// Resource Group contains a zpool resource together with all of the Lustre resources that depend
/// on it.
#[derive(Debug)]
pub struct ResourceGroup {
    pub root: Resource,
    overall_status: Mutex<ResourceStatus>,
}

impl ResourceGroup {
    pub fn new(root: Resource) -> Self {
        assert!(root.kind == "heartbeat/ZFS");
        Self {
            root,
            overall_status: Mutex::new(ResourceStatus::Unknown),
        }
    }
    pub async fn main_loop(&self, args: &crate::commands::Cli) {
        if args.manage_resources {
            self.manage_loop(args).await
        } else {
            self.observe_loop(args).await
        }
    }

    /// This is the main loop for tracking a resource's life cycle in Manage mode. In this mode,
    /// the manager actively starts, and fails over resources to keep them alive.
    ///
    /// A resource starts out in ResourceState::Unknown. As monitor, start, and stop operations are
    /// performed on that resource, across both its home and away hosts, this function tracks that
    /// state.
    async fn manage_loop(&self, args: &crate::commands::Cli) {
        let high_availability = self.root.failover_node.is_some();

        match high_availability {
            true => self.manage_ha(args).await,
            false => self.manage_non_ha(args).await,
        };
    }

    async fn manage_non_ha(&self, _args: &crate::commands::Cli) -> ! {
        self.update_resources(Location::Home).await;
        loop {
            match self.get_overall_status() {
                ResourceStatus::Unknown => self.update_resources(Location::Home).await,
                ResourceStatus::Stopped => self.try_start_resources(Location::Home).await,
                ResourceStatus::RunningOnHome => self.update_resources(Location::Home).await,
                ResourceStatus::RunningOnAway => {
                    panic!("RunningOnAway shouldn't be reachable in a non-HA cluster.")
                }
                ResourceStatus::Unrunnable => {}
                ResourceStatus::CheckingHome => panic!("CheckingHome shouldn't be reachable here."),
                ResourceStatus::CheckingAway => {
                    panic!("CheckingAway shouldn't be reachable in a non-HA cluster.")
                }
            };
            self.update_overall_status();
            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
        }
    }

    /// Check the statuses of each of the resources in this resource group.
    ///
    /// This function updates the status of each resource (zpool and target) in the resource
    /// group, and the host.
    async fn update_resources(&self, loc: Location) {
        let futures = self
            .resources()
            .map(|r| async move { (r, r.monitor(loc).await) });

        let statuses = future::join_all(futures).await;

        let mut error_seen = false;
        for (resource, status) in statuses.iter() {
            match status {
                Ok(monitor_res) => {
                    match monitor_res {
                        ocf::Status::Success => resource.set_status(ResourceStatus::RunningOnHome),
                        ocf::Status::ErrNotRunning => resource.set_status(ResourceStatus::Stopped),
                        // XXX: connection timed out is probably caught in this branch?
                        // needs to set error_seen to true?
                        // need better error model...
                        _ => resource.set_status(ResourceStatus::Unknown),
                    };
                }
                Err(_) => {
                    resource.set_status(ResourceStatus::Unknown);
                    error_seen = true;
                }
            }
        }
        if error_seen {
            self.root.home_node.set_status(HostStatus::Unknown);
        } else {
            self.root.home_node.set_status(HostStatus::Up);
        }
    }

    /// Attempt to start the resources in this resource group on the given location.
    async fn try_start_resources(&self, loc: Location) {
        self.root.start_if_needed_recursive(loc).await;
    }

    fn get_overall_status(&self) -> ResourceStatus {
        *self.overall_status.lock().unwrap()
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

    async fn observe_loop(&self, args: &crate::commands::Cli) {
        let futures = self.resources().map(|r| r.observe_loop(args));
        let _ = future::join_all(futures).await;
    }

    pub fn resources(&self) -> ResourceIterator {
        ResourceIterator {
            queue: VecDeque::from([&self.root]),
        }
    }
}

/// Implementations for a ResourceGroup with a failover host
impl ResourceGroup {
    /// Main loop for managing a ResourceGroup with a failover host
    async fn manage_ha(&self, _args: &crate::commands::Cli) -> ! {
        let loc = self.check_location().await;
        loop {}
    }

    /// Check if the ResourceGroup's root resource is running on either of its hosts.
    async fn check_location(&self) -> Option<Location> {
        match self.root.monitor(Location::Home) {
            _ => todo!(),
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
        let Some(res) = self.queue.pop_front() else {
            return None;
        };

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
    ) -> Self {
        Resource {
            kind: res.kind,
            parameters: res.parameters,
            dependents,
            status: Mutex::new(ResourceStatus::Unknown),
            home_node,
            failover_node,
            context,
        }
    }

    /// This is the loop for tracking a resource's life cycle in Observe mode, where the manager
    /// only checks on resource state and does not actively start / stop a resource.
    async fn observe_loop(&self, args: &crate::commands::Cli) -> ! {
        loop {
            let new_status = self.monitor(Location::Home).await;
            let mut old_status = self.status.lock().unwrap();
            *old_status = match &new_status {
                Ok(s) => match *s {
                    ocf::Status::Success => ResourceStatus::RunningOnHome,
                    ocf::Status::ErrNotRunning => ResourceStatus::Stopped,
                    _ => ResourceStatus::Unknown,
                },
                Err(e) => {
                    if args.verbose {
                        eprintln!("Could not monitor {:?}: {}\n", self, e);
                    }
                    ResourceStatus::Unknown
                }
            };
            std::mem::drop(old_status);
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }

    /// Recursively start a resource as well as all of its dependents.
    /// Updates the status of each resource based on the outcome of the start attempt.
    async fn start_if_needed_recursive(&self, loc: Location) {
        // If this resource is already running, don't bother doing anything:
        if !self.is_running() {
            match self.start(loc).await {
                Ok(status) => match status {
                    ocf::Status::Success => {
                        self.set_running_on_loc(loc);
                    }
                    _ => self.set_status(ResourceStatus::Stopped),
                },
                Err(_) => self.set_status(ResourceStatus::Unknown),
            };
        }

        // Only start the dependents of this resource if it actually started succesfully:
        if self.is_running() {
            let futures = self
                .dependents
                .iter()
                .map(|r| r.start_if_needed_recursive(loc));
            future::join_all(futures).await;
        }
    }

    /// Perform a monitor RPC for this resource.
    pub async fn monitor(&self, loc: Location) -> Result<ocf::Status, Box<dyn Error>> {
        tokio::task::LocalSet::new()
            .run_until(async {
                let reply =
                    do_ocf_request(&self, loc, ocf_resource_agent::Operation::Monitor).await?;
                let status = reply.get()?.get_result()?;
                match status.which() {
                    Ok(ocf_resource_agent::result::Ok(st)) => {
                        let st: ocf::Status = st.into();
                        Ok(st)
                    }
                    Ok(ocf_resource_agent::result::Err(e)) => {
                        let err_str = e?.to_str()?;
                        println!("Remote agent returned error: {err_str}");
                        // XXX: return an actual Err(_) here?
                        Ok(ocf::Status::ErrGeneric)
                    }
                    Err(::capnp::NotInSchema(_)) => {
                        eprintln!("unknown result");
                        Ok(ocf::Status::ErrUnimplemented)
                    }
                }
            })
            .await
    }

    /// Perform a start RPC for this resource.
    pub async fn start(&self, loc: Location) -> Result<ocf::Status, Box<dyn Error>> {
        tokio::task::LocalSet::new()
            .run_until(async {
                let reply =
                    do_ocf_request(&self, loc, ocf_resource_agent::Operation::Start).await?;
                let status = reply.get()?.get_result()?;
                match status.which() {
                    Ok(ocf_resource_agent::result::Ok(st)) => {
                        let st: ocf::Status = st.into();
                        Ok(st)
                    }
                    Ok(ocf_resource_agent::result::Err(e)) => {
                        let e = e?.to_str()?;
                        println!("Remote agent returned error: {e}");
                        Ok(ocf::Status::ErrGeneric)
                    }
                    Err(::capnp::NotInSchema(_)) => {
                        eprintln!("unknown result");
                        Ok(ocf::Status::ErrUnimplemented)
                    }
                }
            })
            .await
    }

    /// Perform a stop RPC for this resource.
    pub async fn stop(&self) -> Result<ocf::Status, Box<dyn Error>> {
        tokio::task::LocalSet::new()
            .run_until(async {
                let reply =
                    do_ocf_request(&self, Location::Home, ocf_resource_agent::Operation::Stop)
                        .await?;
                let status = reply.get()?.get_result()?;
                match status.which() {
                    Ok(ocf_resource_agent::result::Ok(st)) => {
                        let st: ocf::Status = st.into();
                        Ok(st)
                    }
                    Ok(ocf_resource_agent::result::Err(e)) => {
                        let e = e?.to_str()?;
                        println!("Remote agent returned error: {e}");
                        Ok(ocf::Status::ErrGeneric)
                    }
                    Err(::capnp::NotInSchema(_)) => {
                        eprintln!("unknown result");
                        Ok(ocf::Status::ErrUnimplemented)
                    }
                }
            })
            .await
    }

    /// Given the result of a monitor operation--which could have either succesfully returned an
    /// OCF status (like running, not running, etc.) or failed due to a network error, etc.--
    /// update the status of this resource based on that result.
    pub fn update_status(&self, status: Result<ocf::Status, Box<dyn Error>>) {
        match status {
            Ok(monitor_res) => {
                match monitor_res {
                    ocf::Status::Success => self.set_status(ResourceStatus::RunningOnHome),
                    ocf::Status::ErrNotRunning => self.set_status(ResourceStatus::Stopped),
                    // XXX: connection timed out is probably caught in this branch?
                    // needs to set error_seen to true?
                    // need better error model...
                    _ => self.set_status(ResourceStatus::Unknown),
                };
            }
            Err(_) => {
                self.set_status(ResourceStatus::Unknown);
            }
        };
    }

    pub fn get_status(&self) -> ResourceStatus {
        *self.status.lock().unwrap()
    }

    pub fn set_status(&self, status: ResourceStatus) {
        let mut old_status = self.status.lock().unwrap();
        let old_status_copy = *old_status;
        *old_status = status;
        std::mem::drop(old_status);
        if self.context.args.verbose {
            if old_status_copy != status {
                let _ = self.context.out_stream.writeln(
                    self.status_update_string(old_status_copy, status)
                        .as_bytes(),
                );
            }
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
        match self.get_status() {
            ResourceStatus::RunningOnHome | ResourceStatus::RunningOnAway => true,
            _ => false,
        }
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
        params.sort_by(|a, b| a.cmp(b));
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
}

/// The ordering on ResourceStatus is used to rank statuses from "worst" to "best". Statuses that
/// are "worse" should appear first in the enum.
///
/// This ordering is used to determine the "overall" status to assign to a ResourceGroup, when the
/// members of that ResourceGroup may each have their own separate status. If all but one member
/// are RunningOnHome, but one member is Stopped, the group should be considered stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResourceStatus {
    Unknown,
    Unrunnable,
    Stopped,
    CheckingAway,
    CheckingHome,
    RunningOnAway,
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

#[derive(Debug, Clone, Copy)]
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
                vec![ResourceStatus::Unknown, ResourceStatus::Unrunnable].into_iter()
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
