// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

//! Management of a failover cluster with HA pairs.

use std::{
    future::Future,
    hash::{Hash, Hasher},
    io,
    pin::Pin,
    rc::Rc,
};

use {
    futures::{future, stream::FuturesUnordered, StreamExt},
    log::{debug, warn},
};

use crate::{
    halo_capnp::*,
    remote::ocf,
    resource::{Location, ManagementError},
    Cluster,
};

use super::*;

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
}

fn new_message(rg: ResourceToken, kind: Message) -> HostMessage {
    HostMessage::Resource(ResourceMessage {
        resource_group: rg,
        kind,
    })
}

/// Mutable state related to the ongoing management of the Host.
struct HostState {
    /// The set of resources that should be managed on this Host, but are not yet. When Host
    /// management begins, this will be drained and the ResourceToken passed to the management
    /// routine of that ResourceGroup.
    manage_these_resources: Vec<ResourceToken>,

    /// The set of resources that should be checked on this Host, but cannot yet because there was
    /// no active connection when the CheckResourceGroup command came in. When a connection is
    /// established to the remote agent, checking can proceed.
    check_these_resources: Vec<ResourceToken>,

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
    resources_in_transit: Vec<ResourceToken>,

    /// This holds resources that have errors which prevent the management service from managing
    /// them. This includes errors that generally require admin intervention to resolve, for
    /// example a typo in the config file meaning that resource operations fail with "File not
    /// found".
    resources_with_errors: Vec<ResourceToken>,
}

impl HostState {
    pub fn new() -> Self {
        Self {
            manage_these_resources: Vec::new(),
            check_these_resources: Vec::new(),
            outstanding_resource_tasks: HashSet::new(),
            failover_requested: false,
            resources_in_transit: Vec::new(),
            resources_with_errors: Vec::new(),
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
        let mut state = HostState::new();

        // Check whether this host's primary resources are running locally, in order to determine if
        // they should be managed locally or if the failover partner needs to check if they are
        // failed over.
        state.manage_these_resources = self.startup(cluster).await;

        loop {
            match get_client(&self.address()).await {
                Ok(client) => {
                    debug!(
                        "Host {} established connection to its remote agent.",
                        self.id()
                    );
                    let mut client = Rc::new(client);
                    loop {
                        self.remote_connected_loop(client, cluster, &mut state)
                            .await;
                        // remote_connected_loop() only returns once a failover has been requested, and
                        // all host tasks are cancelled.
                        match self.maybe_do_failover(&mut state, cluster).await {
                            // If maybe_do_failover() returned a Client (because it was able to
                            // re-establish connection), we can use that client to re-enter the
                            // remote_connected_loop().
                            Some(new_client) => client = Rc::new(new_client),
                            None => break,
                        };
                    }
                }
                Err(_e) => {
                    debug!(
                        "Host {} failed to establish connection to its remote agent.",
                        self.id()
                    );
                    self.remote_disconnected_loop(cluster, &mut state).await;
                }
            };
        }
    }

    /// When a connection to the remote agent is not possible, the Host task will periodically
    /// attempt to reconnect. If a reconnection attempt succeeds, then this routine exits.
    ///
    /// In the meantime, also listen for messages from the partner host task as well as the admin
    /// CLI utility, and handle them.
    async fn remote_disconnected_loop(&self, cluster: &Cluster, state: &mut HostState) {
        tokio::select! {
            _ = self.remote_liveness_check() => {}
            _ = self.handle_messages_remote_disconnected(cluster, state) => {}
        }
    }

    async fn remote_liveness_check(&self) {
        loop {
            if get_client(&self.address()).await.is_ok() {
                return;
            }

            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
        }
    }

    async fn handle_messages_remote_disconnected(&self, cluster: &Cluster, state: &mut HostState) {
        let home_message =
            "Cannot determine resource status because connection failed to its home host.";
        let away_message =
            "Cannot determine resource status because connection failed to its failover host.";
        let failback_message = format!(
            "Warning: Failback command received by host {} but remote is disconnected.",
            self.id()
        );

        loop {
            match self.receive_message().await {
                HostMessage::Resource(message) => match message.kind {
                    Message::ManageResourceGroup => {
                        let rg = cluster.get_resource_group(&message.resource_group.id);
                        rg.root.set_error_recursive(home_message.to_string());
                        state.manage_these_resources.push(message.resource_group);
                    }
                    Message::CheckResourceGroup => {
                        let rg = cluster.get_resource_group(&message.resource_group.id);
                        rg.root.set_error_recursive(away_message.to_string());
                        state.check_these_resources.push(message.resource_group);
                    }
                    other => {
                        panic!("Unexpected message type {other:?} in client disconnected routine.");
                    }
                },
                HostMessage::Command(command) => match command {
                    HostCommand::Failback => warn!("{}", failback_message),
                },
                HostMessage::None => {
                    panic!("Unexpected message type 'None' in client disconnected routine.")
                }
            }
        }
    }

    async fn remote_connected_loop(
        &self,
        client: Rc<ocf_resource_agent::Client>,
        cluster: &Cluster,
        state: &mut HostState,
    ) {
        // Create a queue of tasks related to this host's management duties.
        // TODO: This Pin<Box<_>> stuff is gross... Can I use `Either` instead and make this nicer?
        let mut tasks: FuturesUnordered<Pin<Box<dyn Future<Output = HostMessage>>>> =
            FuturesUnordered::new();

        // Push a listening task so that the host receives messages, which includes both messages
        // from its failover partner (like "check if my resources are failed over to you"), as well
        // as messages from child tasks (like "connection timed out; failover needed").
        tasks.push(Box::pin(self.receive_message()));

        // Create a task to manage each resource group that should run on this host.
        for token in std::mem::take(&mut state.manage_these_resources) {
            let id = token.id.clone();
            tasks.push(Box::pin(self.manage_resource_group(
                cluster,
                token,
                Rc::clone(&client),
            )));
            state.outstanding_resource_tasks.insert(id);
        }

        // Create a task to check on each resource group that wasn't running on the partner host.
        for token in std::mem::take(&mut state.check_these_resources) {
            tasks.push(Box::pin(self.away_startup_check(
                token,
                cluster,
                Rc::clone(&client),
            )));
        }

        while let Some(event) = tasks.next().await {
            debug!("Host {} got event: {event:?}", self.id());
            match event {
                HostMessage::Command(command) => {
                    match command {
                        HostCommand::Failback => self.do_failback(state, cluster).await,
                    };

                    tasks.push(Box::pin(self.receive_message()));
                }
                HostMessage::Resource(event) => {
                    let id = event.resource_group.id.clone();

                    match event.kind {
                        // Failover partner told this Host to check on this resource group. If they are
                        // running already, this Host proceeds to manage them; otherwise, the original Host
                        // should manage them.
                        Message::CheckResourceGroup => {
                            tasks.push(Box::pin(self.away_startup_check(
                                event.resource_group,
                                cluster,
                                Rc::clone(&client),
                            )));
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
                                Rc::clone(&client),
                            )));
                            state.outstanding_resource_tasks.insert(id);
                            tasks.push(Box::pin(self.receive_message()));
                        }
                        // Child task for management of this resource group encountered an error indicating
                        // that this Host should be fenced, and resources currently on it should be failed
                        // over.
                        Message::RequestFailover => {
                            if self
                                .request_failover(state, cluster, event.resource_group)
                                .await
                            {
                                return;
                            }
                        }
                        // Child task for management of this resource exited after being instructed to
                        // cancel management.
                        Message::TaskCanceled => {
                            if state.failover_requested
                                && self
                                    .request_failover(state, cluster, event.resource_group)
                                    .await
                            {
                                return;
                            }

                            panic!("Unexpected to receive TaskCanceled event when failover was not already requested.");
                        }
                        Message::SwitchHost => {
                            match state
                                .outstanding_resource_tasks
                                .remove(&event.resource_group.id)
                            {
                                true => {
                                    tasks.push(Box::pin(self.switch_host(
                                        event.resource_group,
                                        Rc::clone(&client),
                                        cluster,
                                    )));
                                }
                                false => {
                                    debug!("Rceived SwitchHost message for resource {} on host {}, but it is not currently being managed.", &event.resource_group.id, self.id());
                                }
                            };
                        }
                        Message::ResourceError => {
                            state.resources_with_errors.push(event.resource_group);
                        }
                    };
                }
                HostMessage::None => {}
            }
        }
    }

    /// Returns whether the failover is done or not.
    ///
    /// This is needed so that the loop in remote_connected_loop() knows whether to break out to the
    /// "top level" loop in manage_ha().
    // TODO: can I come up with a cleaner way to do that?
    async fn request_failover(
        &self,
        state: &mut HostState,
        cluster: &Cluster,
        rg: ResourceToken,
    ) -> bool {
        let removed = state.outstanding_resource_tasks.remove(&rg.id);
        if !removed {
            panic!("Unexpected to receive a failover request for a task not oustanding.");
        }

        state.resources_in_transit.push(rg);

        if state.outstanding_resource_tasks.is_empty() {
            // If every resource management task has been cancelled, fencing should proceed:
            true
        } else {
            // If there are outstanding resource group tasks, they need to be notified to exit.
            // However, they must only be notified once, so only do this if this is the first task
            // to request failover.
            if !state.failover_requested {
                for rg in state.outstanding_resource_tasks.iter() {
                    let rg = cluster.get_resource_group(rg);
                    rg.cancel.notify_one();
                    debug!("request failover notified {}", rg.id());
                }
            }

            state.failover_requested = true;
            false
        }
    }

    /// When a connection has been lost to the remote agent, the Host task evaluates whether
    /// failover is required.
    ///
    ///   - is the already_fenced flag set on this Host -- or its partner?
    ///     (TODO: implement this flag... ;)
    ///
    ///   - is the issue temporary? can a connection be re-established?
    ///
    /// In the above cases, do not proceed with fencing.
    ///
    /// If a connection is re-established to the client, return it here so that it can be used to
    /// re-enter the management loop.
    async fn maybe_do_failover(
        &self,
        state: &mut HostState,
        cluster: &Cluster,
    ) -> Option<ocf_resource_agent::Client> {
        let mut tries = 2;

        while tries > 0 {
            debug!(
                "Trying to reconnect to remote agent at {}, attempt {tries}",
                self.id()
            );
            match get_client(&self.address()).await {
                // If we were able to re-establish connection to the client, then return and let
                // the manager try again to manage the resources that were running on this Host.
                Ok(client) => {
                    state.manage_these_resources = std::mem::take(&mut state.resources_in_transit);
                    return Some(client);
                }
                // If an error occurred, then the type of error informs the course of action...
                Err(e) => match e.kind() {
                    // Timed out suggests the Host is down. Proceed with fencing.
                    io::ErrorKind::TimedOut => {}
                    // Any other kind of error suggests the Host is reachable, but there is
                    // likely a configuration issue, like a firewall rule was added that blocks
                    // communication, or the remote daemon was killed.
                    //
                    // It does not make sense to proceed with fencing in this case; instead, the
                    // admin must intervene to correct the issue. Unless this is running in the
                    // test environment, where such errors are intended to result in fencing the
                    // test agent.
                    other => {
                        if !cluster.context.args.fence_on_connection_close {
                            debug!(
                                "Unexpected error '{other}' while trying to reconnect to remote agent at {}.",
                                self.id()
                            );

                            // Once the remote node is healthy again, we want the manager to
                            // proceed with managing the resources again.
                            state.manage_these_resources =
                                std::mem::take(&mut state.resources_in_transit);
                            return None;
                        }
                    }
                },
            }

            tries -= 1;
        }

        self.do_failover(state).await;

        None
    }

    async fn do_failover(&self, state: &mut HostState) {
        self.do_fence(FenceCommand::Off)
            .expect("Fencing failed... TODO: handle this case...");

        warn!("Host {} has been powered off.", self.id());

        for mut rg in state.resources_in_transit.drain(..) {
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

    async fn do_failback(&self, state: &mut HostState, cluster: &Cluster) {
        for res_id in state.outstanding_resource_tasks.iter() {
            let rg = cluster.get_resource_group(res_id);

            // If a resource is currently home, there is nothing to do for it.
            if rg.root.home_node.id() == self.id() {
                continue;
            }

            // If a resource is not home, then need to stop it and pass management on...
            warn!("{res_id} is not home and will be moved back.");

            rg.switch_host.notify_one();
        }
    }

    async fn switch_host(
        &self,
        mut token: ResourceToken,
        client: Rc<ocf_resource_agent::Client>,
        cluster: &Cluster,
    ) -> HostMessage {
        let rg = cluster.get_resource_group(&token.id);

        match rg.stop_resources(&client).await {
            Ok(()) => {}
            Err(ManagementError::Configuration) => {
                debug!("Switch host operation recieved unexpected configuration error from remote agent.");
                return new_message(token, Message::ResourceError);
            }
            Err(ManagementError::Connection) => {
                debug!("Connection to remote agent failed during switch host operation.");
                return new_message(token, Message::RequestFailover);
            }
        };

        let partner = self.failover_partner().unwrap();

        match token.location {
            Location::Home => token.location = Location::Away,
            Location::Away => token.location = Location::Home,
        };

        partner
            .sender
            .send(new_message(token, Message::ManageResourceGroup))
            .await
            .unwrap();

        HostMessage::None
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
    ///     - if it is discovered to be running, return the token to the caller, who will arrange
    ///       for the ResourceGroup management routine to run
    ///     - if it is not running, send a message to the failover partner to check its status
    ///       there.
    ///     - the failover partner will either discover it to be running there and start managing
    ///       it, or will send a message back to this host to start managing it.
    ///
    /// If a connection to the remote agent for this Host cannot be established, then just send
    /// a message to the failover partner to see if the ResourceGroup is running there.
    async fn startup(&self, cluster: &Cluster) -> Vec<ResourceToken> {
        // Mint tokens for each resource group:
        let my_resources = cluster
            .host_home_resource_groups(self)
            .map(|rg| ResourceToken {
                id: rg.id().to_string(),
                location: Location::Home,
            });

        let (manage_these, send_these): (Vec<ResourceToken>, Vec<ResourceToken>) =
            match get_client(&self.address()).await {
                Ok(client) => {
                    let mut manage_these = Vec::new();
                    let mut send_these = Vec::new();

                    let statuses =
                        my_resources.map(|token| self.home_startup_check(token, cluster, &client));

                    for (token, is_home) in future::join_all(statuses).await {
                        if is_home {
                            manage_these.push(token)
                        } else {
                            send_these.push(token)
                        }
                    }

                    (manage_these, send_these)
                }
                Err(_) => (Vec::new(), my_resources.collect()),
            };

        let partner = self
            .failover_partner()
            .expect("Host without failover partner in HA routine.");

        for mut token in send_these {
            token.location = Location::Away;
            let message = new_message(token, Message::CheckResourceGroup);
            partner.sender.send(message).await.unwrap();
        }

        manage_these
    }

    /// Perform startup check for a resource on its home node.
    /// Returns (_, true) if the resource is running on the home node.
    async fn home_startup_check(
        &self,
        token: ResourceToken,
        cluster: &Cluster,
        client: &ocf_resource_agent::Client,
    ) -> (ResourceToken, bool) {
        let rg = cluster.get_resource_group(&token.id);
        let status = remote_ocf_operation_given_client(
            &rg.root,
            client,
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
                (token, true)
            }
            ocf::Status::Error(error_type, message) => {
                match error_type {
                    ocf::OcfError::ErrNotRunning => (token, false),
                    other => panic!("Remote agent gave unexpected status: {other:?}: {message}. TODO: handle this..."),
                }
            }
        }
    }

    /// Perform startup logic for a single resource group on its failover node.
    ///
    /// Checks if the resource group appears to be running on the failover node.
    ///
    /// - If yes, this sends a Message::ManageResourceGroup message to self, to direct this Host
    ///   task to begin managing the resource group.
    ///
    /// - If not, sends a ManageResourceGroup message back to the home node to tell that node to
    ///   assume management of the resource.
    ///
    /// - If an error was observed, returns a Message::ResourceError to inform the main Host task of
    ///   the situation.
    async fn away_startup_check(
        &self,
        mut token: ResourceToken,
        cluster: &Cluster,
        client: Rc<ocf_resource_agent::Client>,
    ) -> HostMessage {
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
                debug!("Remote agent returned error: {message}");
                return new_message(token, Message::ResourceError);
            }
        };

        match status {
            ocf::Status::Success => {
                return new_message(token, Message::ManageResourceGroup);
            }
            ocf::Status::Error(error_type, message) => match error_type {
                ocf::OcfError::ErrNotRunning => {}
                other => {
                    debug!("Remote agent returned error: {other:?}: {message}");
                    return new_message(token, Message::ResourceError);
                }
            },
        };

        let partner = self
            .failover_partner()
            .expect("Host without failover partner in HA management routine.");

        token.location = Location::Home;
        let message = new_message(token, Message::ManageResourceGroup);
        partner.sender.send(message).await.unwrap();

        HostMessage::None
    }

    /// Management of a resource group proceeds by calling the management loop method on
    /// ResourceGroup. At the same time, however, this task must be cancellable in case management
    /// should end for any reason, so it also listens for the cancel signal.
    async fn manage_resource_group(
        &self,
        cluster: &Cluster,
        token: ResourceToken,
        client: Rc<ocf_resource_agent::Client>,
    ) -> HostMessage {
        let rg = cluster.get_resource_group(&token.id);

        debug!("Host {} is managing resource group {}", self.id(), rg.id());

        tokio::select! {
            // Biased because if a task has been cancelled, it should exit ASAP and not bother
            // trying to do any more work.
            biased;

            // Received a cancel notification: exit right away.
            _ = rg.cancel.notified() => {
                new_message(token, Message::TaskCanceled)
            }

            _ = rg.switch_host.notified() => {
                new_message(token, Message::SwitchHost)
            }

            // If the resource management loop returns, it must be because it observed an error
            // condition.
            err = rg.manage_loop(&client, token.location) => {
                debug!(
                    "{} received error '{err:?}' for resource group {}",
                    self.id(),
                    rg.id()
                );

                match err {
                    ManagementError::Connection => new_message(token, Message::RequestFailover),
                    ManagementError::Configuration => {
                        debug!("host {} got a configuration error when managing resource group {}", self.id(), token.id);
                        new_message(token, Message::ResourceError)
                    }
                }
            }
        }
    }
}
