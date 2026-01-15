// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

//! Management of a failover cluster with HA pairs.

use std::{
    cell::Ref,
    future::Future,
    hash::{Hash, Hasher},
    pin::Pin,
};

use futures::{future::JoinAll, stream::FuturesUnordered, StreamExt};

use crate::{halo_capnp::*, remote::ocf, resource::Location, Cluster};

use super::*;

#[derive(Debug)]
pub struct HostMessage {
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

    /// A resource management task has observed a condition like "connection timed out" and
    /// failover should be triggered.
    RequestFailover,

    /// A resource management task has received a cancellation request and has exited.
    TaskCanceled,
}

fn new_message(rg: ResourceToken, kind: Message) -> HostMessage {
    HostMessage {
        resource_group: rg,
        kind,
    }
}

// Mutable state related to the ongoing management of the Host.
struct HostState {
    /// Tracker for the outstanding child tasks that are managing resources on this
    /// host. This is used for making sure that every outstanding task is cancelled before an
    /// action occurs that results in new management tasks being launched. This includes both
    /// fencing, and a TCP connection breaking resulting in a new client being needed.
    outstanding_resource_tasks: HashSet<String>,

    /// If a resource management task returns with an error indicating failover should occur, we
    /// need to wait on the remaining tasks to be canceled. Once every outstanding task has
    /// returned, failover can begin.
    failover_requested: bool,

    /// When a failover action has been requested, this set is filled up with the IDs of all the
    /// resource groups that were running on the Host. This needs to be tracked so that the correct
    /// resource groups are sent over to the failover partner for management.
    resources_in_transit: HashSet<ResourceToken>,
}

impl HostState {
    pub fn new() -> Self {
        Self {
            outstanding_resource_tasks: HashSet::new(),
            failover_requested: false,
            resources_in_transit: HashSet::new(),
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

impl PartialEq for ResourceToken {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for ResourceToken {}

impl Hash for ResourceToken {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

#[allow(clippy::await_holding_refcell_ref)]
impl Host {
    /// The main management loop for managing a particular host.
    ///
    /// When the system starts, each Host task is responsible for determining the state of its
    /// primary resources.
    ///
    /// - If the resource is discovered to be running on the primary, that host management loop will
    ///   proceed to management of it.
    /// - If the resource is not currently running on the primary, the failover partner needs to be
    ///   directed to check on its status.
    ///   - If the resource is discovered to be running on the secondary, it will proceed to the
    ///     main  management loop for the resource.
    ///   - If the resource is not running anywhere, the secondary will send a message back to the
    ///     primary telling the primary to proceed with management.
    ///
    /// If an error occurs (like "Connection Timed Out"), the host task will be responsible for
    /// stopping all management activities on that host, fencing the host, and then notifying the
    /// partner host's task to assume management of those resources.
    pub async fn manage_ha(&self, cluster: &Cluster) {
        let client = ClientWrapper::new(self.address().to_string());

        let mut state = HostState::new();

        // Create a queue of tasks related to this host's management duties.
        // TODO: This Pin<Box<_>> stuff is gross... Can I use `Either` instead and make this nicer?
        let mut tasks: FuturesUnordered<Pin<Box<dyn Future<Output = HostMessage>>>> =
            FuturesUnordered::new();

        // Push a listening task so that the host receives messages, which includes both messages
        // from its failover partner (like "check if my resources are failed over to you"), as well
        // as messages from child tasks (like "connection timed out; failover needed").
        tasks.push(Box::pin(self.receive_message()));

        // Check whether this host's primary resources are running locally, in order to determine if
        // they should be managed locally or if the failover partner needs to check if they are
        // failed over.
        let manage_these_rgs = self.startup(cluster, client.get().await).await;

        // Create a task to manage each resource group that is currently running on this host.
        for token in manage_these_rgs {
            let id = token.id.clone();
            tasks.push(Box::pin(self.manage_resource_group(
                cluster,
                token,
                client.get().await,
            )));
            state.outstanding_resource_tasks.insert(id);
        }

        // Here's where the real work begins: drive forward all of the tasks added to the queue,
        // and respond to events as they come in.
        while let Some(event) = tasks.next().await {
            println!("Host {} got event: {event:?}", self.id());
            let id = event.resource_group.id.clone();

            match event.kind {
                // Failover partner told this Host to check on this resource group. If they are
                // running already, this Host proceeds to manage them; otherwise, the original Host
                // should manage them.
                Message::CheckResourceGroup => {
                    let client_ref = client.get().await;
                    if let Some(resource_token) = self
                        .startup_one_rg(
                            event.resource_group,
                            cluster,
                            Ref::clone(&client_ref),
                            false,
                        )
                        .await
                    {
                        tasks.push(Box::pin(self.manage_resource_group(
                            cluster,
                            resource_token,
                            client_ref,
                        )));
                        state.outstanding_resource_tasks.insert(id);
                    };
                    // TODO: rather than have to duplicate this "re-arming" of the receive message
                    // task in every branch that needs it, can I come up with a way to distinguish
                    // in a single place that it needs re-arming? (`Either` might help here...)
                    tasks.push(Box::pin(self.receive_message()));
                }
                // Partner Host told this Host to begin managing this resource group.
                Message::ManageResourceGroup => {
                    tasks.push(Box::pin(self.manage_resource_group(
                        cluster,
                        event.resource_group,
                        client.get().await,
                    )));
                    state.outstanding_resource_tasks.insert(id);
                    tasks.push(Box::pin(self.receive_message()));
                }
                // Child task for management of this resource group encountered an error indicating
                // that this Host should be fenced, and resources currently on it should be failed
                // over.
                Message::RequestFailover => {
                    self.request_failover(&mut state, cluster, event.resource_group, &client)
                        .await;
                }
                // Child task for management of this resource exited after being instructed to
                // cancel management.
                Message::TaskCanceled => {
                    if state.failover_requested {
                        self.request_failover(&mut state, cluster, event.resource_group, &client)
                            .await;
                        continue;
                    }

                    panic!("Unexpected to receive TaskCanceled event when failover was not already requested.");
                }
            };
        }
    }

    async fn request_failover(
        &self,
        state: &mut HostState,
        cluster: &Cluster,
        rg: ResourceToken,
        client: &ClientWrapper,
    ) {
        let removed = state.outstanding_resource_tasks.remove(&rg.id);
        if !removed {
            panic!("Unexpected to receive a failover request for a task not oustanding.");
        }

        state.resources_in_transit.insert(rg);

        if state.outstanding_resource_tasks.is_empty() {
            // If every resource management task has been cancelled, fencing should proceed:
            self.do_failover(state, client).await;
        } else {
            // If there are outstanding resource group tasks, they need to be notified to exit.
            // However, they must only be notified once, so only do this if this is the first task
            // to request failover.
            if !state.failover_requested {
                for rg in state.outstanding_resource_tasks.iter() {
                    let rg = cluster.get_resource_group(rg);
                    rg.cancel.notify_one();
                }
            }

            state.failover_requested = true;
        }
    }

    async fn do_failover(&self, state: &mut HostState, client: &ClientWrapper) {
        self.do_fence(FenceCommand::Off)
            .expect("Fencing failed... TODO: handle this case...");

        println!("Host {} did fence off...", self.id());

        client.clear();

        for mut rg in state.resources_in_transit.drain() {
            let partner = self.failover_partner().unwrap();
            match rg.location {
                Location::Home => rg.location = Location::Away,
                Location::Away => rg.location = Location::Home,
            };

            partner
                .sender
                .send(new_message(rg, Message::ManageResourceGroup))
                .await
                .unwrap();
        }
    }

    /// The purpose of this procedure is to perform startup logic to discover the existing state of
    /// resources when the management service starts.
    ///
    /// It begins by minting a ResourceToken for each ResourceGroup whose home is this Host. This
    /// is the only place that ResourceTokens should be created - and they must never be
    /// destroyed.
    ///
    /// - for each ResourceGroup:
    ///     - Query the status of the resoource group on this host
    ///     - if it is discovered to be running, proceed to management loop
    ///     - if it is not running, send a message to the failover partner to check its status
    ///       there.
    ///     - the failover partner will either discover it to be running there and start managing
    ///       it, or will send a message back to this host to start managing it.
    async fn startup(
        &self,
        cluster: &Cluster,
        client: Ref<'_, ocf_resource_agent::Client>,
    ) -> Vec<ResourceToken> {
        // Mint tokens for each resource group:
        let my_resources = cluster
            .host_home_resource_groups(self)
            .map(|rg| ResourceToken {
                id: rg.id().to_string(),
                location: Location::Home,
            });

        let statuses: JoinAll<_> = my_resources
            .map(|token| self.startup_one_rg(token, cluster, Ref::clone(&client), true))
            .collect();

        statuses.await.into_iter().flatten().collect()
    }

    /// Perform startup logic for a single resource group.
    ///
    /// Checks if the resource group appears to be running here.
    ///
    /// If yes, returns the token and the host management task knows to launch a resource management
    /// task for this RG.
    ///
    /// If not, the behavior depends on whether we are checking the home node or the away node.
    ///
    /// Checking on home:
    ///     - sends a CheckResourceGroup message to the failover partner telling the partner to check the RG.
    ///     Then the partner will either find that the RG is running there, and begin managing
    ///     it, or will find that the resource is not running, and send a message back to this Host
    ///     so this host knows to start managing it.
    ///
    /// Checking on away:
    ///     - sends a ManageResourceGroup message to the failover partner to tell the partner to
    ///     begin managing the RG.
    async fn startup_one_rg(
        &self,
        mut token: ResourceToken,
        cluster: &Cluster,
        client: Ref<'_, ocf_resource_agent::Client>,
        checking_home: bool,
    ) -> Option<ResourceToken> {
        let rg = cluster.get_resource_group(&token.id);
        let status = remote_ocf_operation_given_client(
            &rg.root,
            &client,
            ocf_resource_agent::Operation::Monitor,
        )
        .await;
        let status = status.expect("FIXME: handle error here...");

        let status = match status {
            AgentReply::Success(status) => status,
            AgentReply::Error(message) => {
                panic!("Remote agent gave unexpected error: {message}. TODO: handle this...")
            }
        };

        match status {
            ocf::Status::Success => {
                return Some(token);
            }
            ocf::Status::ErrNotRunning => {}
            other => panic!("Remote agent gave unexpected status: {other}. TODO: handle this..."),
        };

        // Resource was _not_ running on home: need to check partner node:

        let Some(partner) = self.failover_partner() else {
            println!("no failover partner");
            return Some(token);
        };

        let message = if checking_home {
            token.location = Location::Away;
            new_message(token, Message::CheckResourceGroup)
        } else {
            token.location = Location::Home;
            new_message(token, Message::ManageResourceGroup)
        };

        partner.sender.send(message).await.unwrap();

        None
    }

    /// Management of a resource group proceeds by calling the management loop method on
    /// ResourceGroup. At the same time, however, this task must be cancellable in case management
    /// should end for any reason, so it also listens for the cancel signal.
    async fn manage_resource_group(
        &self,
        cluster: &Cluster,
        token: ResourceToken,
        client: Ref<'_, ocf_resource_agent::Client>,
    ) -> HostMessage {
        let rg = cluster.get_resource_group(&token.id);

        tokio::select! {
            // Biased because if a task has been cancelled, it should exit ASAP and not bother
            // trying to do any more work.
            biased;

            // Received a cancel notification: exit right away.
            _ = rg.cancel.notified() => {
                new_message(token, Message::TaskCanceled)
            }

            // If the resource management loop returns, it must be because it observed an error
            // condition.
            err = rg.manage_loop(&client, token.location) => {
                let err = err.unwrap_err();
                eprintln!(
                    "{} received error {err} for resource group {}",
                    self.id(),
                    rg.id()
                );

                new_message(token, Message::RequestFailover)
            }
        }
    }
}
