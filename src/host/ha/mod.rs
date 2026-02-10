// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use std::{future::Future, pin::Pin, rc::Rc};

use {futures::stream::FuturesUnordered, tokio::sync::Notify};

use crate::{
    cluster::Cluster,
    halo_capnp::*,
    resource::{Location, ManagementError},
};

use super::*;

pub mod manage;
pub mod observe;

/// A set of tasks that the Host management task is currently running.
// TODO: This Pin<Box<_>> stuff is gross... Can I use `Either` instead and make this nicer?
pub type ManagementTasks<'a> = FuturesUnordered<Pin<Box<dyn Future<Output = HostMessage> + 'a>>>;

#[derive(Debug)]
pub enum HostMessage {
    /// A command from the admin, via the CLI utility.
    Command(HostCommand),

    /// A message either from a partner Host task, or a child Resource task, indicating an action
    /// that should be taken for a particular ResourceGroup.
    Resource(ResourceMessage),

    /// A message from a child task indicating that it exited normally, and no further action is
    /// needed from this Host on the ResourceGroup that the task had been managing.
    ///
    /// Normally this would be because the child task passed the ResourceToken over to the partner
    /// host. (If it hadn't passed on the ResourceToken, then the ResourceToken would need to be
    /// returned in a HostMessage::Resource.)
    None,
}

#[derive(Debug)]
pub struct ResourceMessage {
    resource_group: ResourceToken,
    kind: Message,
}

/// The commands that can be sent to a Host management task.
#[derive(Debug)]
enum Message {
    /// Check the status of the resource group to determine if it is running or not.
    CheckResourceGroup,

    /// Begin management of the resource group.
    ManageResourceGroup,

    /// Begin observation of the resource group -- check on its status, but don't start it if
    /// stopped.
    ObserveResourceGroup,

    /// A resource management task has observed a condition like "connection timed out" and
    /// failover should be triggered.
    RequestFailover,

    /// A resource management task has received a cancellation request and has exited.
    TaskCanceled,

    /// A resource management task has been told to stop and pass mangement over to the partner
    /// host.
    SwitchHost,

    /// A resource management task reported that this resource has an error which prevents the
    /// service from managing it.
    ResourceError,
    // TODO: probably need a message for "Unmanage"--when a resource is unmanaged, its management
    // task should be cancelled and a new task launched that will monitor it on both hosts, for
    // cases where the admin manually moves it over to the failover partner.
}

fn new_message(rg: ResourceToken, kind: Message) -> HostMessage {
    HostMessage::Resource(ResourceMessage {
        resource_group: rg,
        kind,
    })
}

/// This object is shared between a parent Host task, and a ResourceGroup task that the Host task
/// has launched. The inner Notify objects are triggered by the Host task when it wants to cancel
/// the Resource task that holds it.
#[derive(Clone)]
struct ResourceTaskCancel {
    /// The ID of the ResourceGroup that this Cancel object is for.
    id: String,

    /// A notification mechanism used to tell a resource group management task to stop because
    /// connection was lost to the remote agent, and a fence action might be initiated.
    lost_connection: Rc<Notify>,

    /// A notification mechanism to tell a resource group management task to stop so that
    /// management can be handed over to the partner host.
    switch_host: Rc<Notify>,
}

impl ResourceTaskCancel {
    fn new(id: String) -> Self {
        Self {
            id,
            lost_connection: Rc::new(Notify::new()),
            switch_host: Rc::new(Notify::new()),
        }
    }
}

/// A ResourceToken represents the current state of a resource management task within the Host
/// management system.
///
/// At any given time, a ResourceToken is either:
/// - held by the management routine (i.e., the `manage_resource_group()` method on `Host`), if the
///   resource group is currently being managed; or
/// - held by a HostMessage object, if the resource is in some kind of transational state, like
///   waiting for failover to occur, or discovering its initial status on manager startup.
///
/// The ResourceToken is passed along from state to state to ensure that no ResourceGroup is ever
/// "dropped" and thus forgotten about.
///
/// To ensure that a ResourceGroup is never forgotten about, the drop() implementation panics, so
/// that it is a runtime error for a ResourceGroup to transition to an unexpected state.
#[derive(Debug)]
struct ResourceToken {
    id: String,
    location: Location,
}

impl Drop for ResourceToken {
    fn drop(&mut self) {
        panic!("Resource token {self:?} was illegally dropped!");
    }
}

impl Host {
    /// Mint a ResourceToken for each ResourceGroup whose home is this Host. This is the only place
    /// that ResourceTokens can be created - and they must never be destroyed.
    fn mint_resource_tokens(&self, cluster: &Cluster) -> Vec<ResourceToken> {
        cluster
            .host_home_resource_groups(self)
            .map(|rg| ResourceToken {
                id: rg.id().to_string(),
                location: Location::Home,
            })
            .collect()
    }

    fn ha_failover_partner(&self) -> &Arc<Host> {
        self.failover_partner()
            .expect("Host without failover partner in HA routine.")
    }

    /// Sends the token over to the partner in the given message type.
    ///
    /// This flips the location field -- the caller should NOT adjust location before calling this!
    async fn send_message_to_partner(&self, mut token: ResourceToken, message: Message) {
        match token.location {
            Location::Home => token.location = Location::Away,
            Location::Away => token.location = Location::Home,
        };

        let partner = self.ha_failover_partner();

        partner
            .sender
            .send(new_message(token, message))
            .await
            .unwrap();
    }
}

/// Determine if a resource is running on the system connected in the given client.
///
/// If communication fails for some reason, an answer cannot be given, so Err(_) is returned
/// instead.
async fn is_resource_group_running_here(
    token: &ResourceToken,
    cluster: &Cluster,
    client: &ocf_resource_agent::Client,
    update_status_if_stopped: bool,
) -> Result<bool, ManagementError> {
    let rg = cluster.get_resource_group(&token.id);

    rg.is_running_here(client, token.location, update_status_if_stopped)
        .await
}
