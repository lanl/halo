// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

//! Management of a failover cluster with HA pairs.

use std::{io, mem::take};

use {
    futures::{future, stream::FuturesUnordered, StreamExt},
    log::{debug, warn},
};

use crate::{
    cluster::Cluster,
    resource::{ManagementError, ResourceStatus},
};

use super::*;

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
    outstanding_resource_tasks: Vec<ResourceTaskCancel>,

    /// When a failover action has been requested, this set is filled up with the IDs of all the
    /// resource groups that were running on the Host. This needs to be tracked so that the correct
    /// resource groups are sent over to the failover partner for management.
    resources_in_transit: Vec<ResourceToken>,

    /// This holds resources that have errors which prevent the management service from managing
    /// them. This includes errors that generally require admin intervention to resolve, for
    /// example a typo in the config file meaning that resource operations fail with "File not
    /// found".
    resources_with_errors: Vec<ResourceToken>,

    admin_requested_fence: bool,
}

impl HostState {
    fn new() -> Self {
        Self {
            manage_these_resources: Vec::new(),
            check_these_resources: Vec::new(),
            outstanding_resource_tasks: Vec::new(),
            resources_in_transit: Vec::new(),
            resources_with_errors: Vec::new(),
            admin_requested_fence: false,
        }
    }

    /// When a ResourceGroup task exits, it needs to remove its ResourceTaskCancel object from
    /// outsanding_resource_tasks.
    // TODO: is this method even necessary? Check the callers, in some / all cases, the resource
    // tasks have already been removed from outstanding_resource_tasks *before* the task itself
    // exits.
    fn resource_task_exited(&mut self, id: &str) {
        let still_running = take(&mut self.outstanding_resource_tasks)
            .into_iter()
            .filter(|task| task.id != id)
            .collect();

        self.outstanding_resource_tasks = still_running;
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

        let my_resources = self.mint_resource_tokens(cluster);

        // Check whether this host's primary resources are running locally, in order to determine if
        // they should be managed locally or if the failover partner needs to check if they are
        // failed over.
        state.manage_these_resources = self.startup(cluster, my_resources).await;

        loop {
            match self.get_client().await {
                Ok(mut client) => {
                    debug!(
                        "Host {} established connection to its remote agent.",
                        self.id()
                    );
                    loop {
                        self.remote_connected_loop(&client, cluster, &mut state)
                            .await;

                        // All resource management tasks must have been cancelled before
                        // remote_connected_loop() returns:
                        assert!(state.outstanding_resource_tasks.is_empty());

                        // If we reached this point, failover must have been requested
                        match self.maybe_do_failover(&mut state, cluster).await {
                            // If maybe_do_failover() returned a Client (because it was able to
                            // re-establish connection), we can use that client to re-enter the
                            // remote_connected_loop().
                            Some(new_client) => client = new_client,
                            None => break,
                        };
                    }
                }
                Err(_e) => {
                    self.set_connected(false);
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
            _ = self.remote_liveness_check(cluster) => {}
            _ = self.handle_messages_remote_disconnected(cluster, state) => {}
        }
    }

    async fn remote_liveness_check(&self, cluster: &Cluster) {
        loop {
            if self.get_client().await.is_ok() {
                return;
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(cluster.args.sleep_time)).await;
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
                        rg.root
                            .set_status_recursive(ResourceStatus::Error(home_message.to_string()));
                        state.manage_these_resources.push(message.resource_group);
                    }
                    Message::CheckResourceGroup => {
                        let rg = cluster.get_resource_group(&message.resource_group.id);
                        rg.root
                            .set_status_recursive(ResourceStatus::Error(away_message.to_string()));
                        state.check_these_resources.push(message.resource_group);
                    }
                    other => {
                        panic!("Unexpected message type {other:?} in client disconnected routine.");
                    }
                },
                HostMessage::Command(command) => match command {
                    HostCommand::Failback => warn!("{}", failback_message),
                    HostCommand::Fence => todo!(),
                    HostCommand::Activate => todo!("activate in disconnected mode"),
                    // Deactivate message: nothing to do because the remote is disconnected. No
                    // ability to stop resources even if they happened to be running on the remote.
                    HostCommand::Deactivate => {}
                },
                HostMessage::None => {
                    panic!("Unexpected message type 'None' in client disconnected routine.")
                }
            }
        }
    }

    async fn remote_connected_loop(
        &self,
        client: &ocf_resource_agent::Client,
        cluster: &Cluster,
        state: &mut HostState,
    ) {
        // Create a set of tasks related to this host's management duties.
        let mut tasks: ManagementTasks = FuturesUnordered::new();

        // Push a listening task so that the host receives messages, which includes both messages
        // from its failover partner (like "check if my resources are failed over to you"), as well
        // as messages from child tasks (like "connection timed out; failover needed").
        tasks.push(Box::pin(self.receive_message()));

        // Create a task to manage each resource group that should run on this host.
        for token in take(&mut state.manage_these_resources) {
            let id = token.id.clone();
            let revoke = ResourceTaskCancel::new(id);
            tasks.push(Box::pin(self.manage_resource_group(
                cluster,
                token,
                client,
                revoke.clone(),
            )));
            state.outstanding_resource_tasks.push(revoke);
        }

        // Create a task to check on each resource group that wasn't running on the partner host.
        for token in take(&mut state.check_these_resources) {
            tasks.push(Box::pin(
                self.check_resource_group_managed(token, cluster, client),
            ));
        }

        while let Some(event) = tasks.next().await {
            debug!("Host {} got event: {event:?}", self.id());
            match event {
                HostMessage::Command(command) => {
                    match command {
                        HostCommand::Failback => self.do_failback(state, cluster),
                        HostCommand::Activate => {}
                        HostCommand::Deactivate => self.deactivate(state),
                        HostCommand::Fence => self.admin_fence_request(state),
                    };

                    tasks.push(Box::pin(self.receive_message()));
                }
                HostMessage::Resource(event) => {
                    let id = &event.resource_group.id;
                    match event.kind {
                        // Failover partner told this Host to check on this resource group. If they are
                        // running already, this Host proceeds to manage them; otherwise, the original Host
                        // should manage them.
                        Message::CheckResourceGroup => {
                            tasks.push(Box::pin(self.check_resource_group_managed(
                                event.resource_group,
                                cluster,
                                client,
                            )));
                            // TODO: rather than have to duplicate this "re-arming" of the receive message
                            // task in every branch that needs it, can I come up with a way to distinguish
                            // in a single place that it needs re-arming? (`Either` might help here...)
                            tasks.push(Box::pin(self.receive_message()));
                        }
                        // Partner Host told this Host to begin managing this resource group.
                        Message::ManageResourceGroup => {
                            let revoke = ResourceTaskCancel::new(id.clone());
                            tasks.push(Box::pin(self.manage_resource_group(
                                cluster,
                                event.resource_group,
                                client,
                                revoke.clone(),
                            )));
                            state.outstanding_resource_tasks.push(revoke);
                            tasks.push(Box::pin(self.receive_message()));
                        }
                        Message::ObserveResourceGroup => {
                            tasks.push(Box::pin(self.observe_resource_group_managed_mode(
                                event.resource_group,
                                cluster,
                                client,
                            )));
                            tasks.push(Box::pin(self.receive_message()));
                        }
                        // Child task for management of this resource group encountered an error indicating
                        // that this Host should be fenced, and resources currently on it should be failed
                        // over.
                        Message::RequestFailover => {
                            state.resource_task_exited(id);
                            if self.ready_for_failover(state, event.resource_group) {
                                return;
                            }
                        }
                        // Child task for management of this resource exited after being instructed to
                        // cancel management.
                        Message::TaskCanceled => {
                            state.resource_task_exited(id);
                            let rg = cluster.get_resource_group(id);
                            rg.root.set_status_recursive(ResourceStatus::Unknown(
                                "Connection lost; currently unmanaged.".to_string(),
                            ));
                            if self.ready_for_failover(state, event.resource_group) {
                                return;
                            }
                        }
                        Message::SwitchHost => {
                            state.resource_task_exited(id);
                            tasks.push(Box::pin(self.switch_host(
                                event.resource_group,
                                client,
                                cluster,
                            )));
                        }
                        Message::ResourceError => {
                            state.resources_with_errors.push(event.resource_group);
                        }
                    };
                }
                HostMessage::None => {}
            }
        }

        unreachable!()
    }

    /// Deactivate this Host:
    ///   - Cancel any outstanding resource management tasks
    ///   - send the ResourceTokens over to the partner
    fn deactivate(&self, state: &mut HostState) {
        for revoke in take(&mut state.outstanding_resource_tasks) {
            debug!(
                "Deactivating host {}: notifying task for resource '{}'",
                self.id(),
                revoke.id
            );
            revoke.switch_host.notify_one();
        }
    }

    /// Returns whether the failover is done or not.
    ///
    /// This is needed so that the loop in remote_connected_loop() knows whether to break out to the
    /// "top level" loop in manage_ha().
    // TODO: can I come up with a cleaner way to do that?
    fn ready_for_failover(&self, state: &mut HostState, rg: ResourceToken) -> bool {
        state.resources_in_transit.push(rg);

        if state.outstanding_resource_tasks.is_empty() {
            // If every resource management task has been cancelled, fencing should proceed:
            true
        } else {
            // If there are outstanding resource group tasks, they need to be notified to exit.
            // However, they must only be notified once, so only do this if this is the first task
            // to request failover.
            for revoke in &state.outstanding_resource_tasks {
                debug!(
                    "request failover: notifiying task for resource '{}'",
                    revoke.id
                );
                revoke.lost_connection.notify_one();
            }

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
        if state.admin_requested_fence {
            state.admin_requested_fence = false;
            self.do_failover(state, cluster).await;
            return None;
        }

        if self.is_fenced() {
            //Need to include logic, either programmatically or admin intervention to set this flag back to false
            warn!("Host {} was already fenced; not fencing again", self.id());
            return None;
        }

        let mut tries = 2;

        while tries > 0 {
            debug!(
                "Trying to reconnect to remote agent at {}, attempt {tries}",
                self.id()
            );
            match self.get_client().await {
                // If we were able to re-establish connection to the client, then return and let
                // the manager try again to manage the resources that were running on this Host.
                Ok(client) => {
                    state.manage_these_resources = take(&mut state.resources_in_transit);
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
                        if !cluster.args.fence_on_connection_close {
                            debug!(
                                "Unexpected error '{other}' while trying to reconnect to remote agent at {}.",
                                self.address()
                            );

                            // Once the remote node is healthy again, we want the manager to
                            // proceed with managing the resources again.
                            //
                            // TODO: Is this safe? Maybe instead of putting these in
                            // manage_these_resources, this should re-do the startup logic, in case
                            // the admin manually moved the resources while the connection to the
                            // remote was lost?
                            state.manage_these_resources = take(&mut state.resources_in_transit);
                            return None;
                        }
                    }
                },
            }

            tries -= 1;
        }

        // do not proceed if the partner is deactivated.
        if !self.ha_failover_partner().active() {
            warn!(
                "Host {} is disconnected, but cannot fail over because partner is deactivated.",
                self.id()
            );
            state.manage_these_resources = take(&mut state.resources_in_transit);
            return None;
        }

        self.do_failover(state, cluster).await;

        None
    }

    async fn do_failover(&self, state: &mut HostState, cluster: &Cluster) {
        self.do_fence_nonblocking(FenceCommand::Off)
            .await
            .expect("Fencing failed... TODO: handle this case...");

        warn!("Host {} has been powered off.", self.id());

        for token in take(&mut state.resources_in_transit) {
            let rg = cluster.get_resource_group(&token.id);
            // We know resource is stopped if fencing succeeded:
            rg.root.set_status_recursive(ResourceStatus::Stopped);

            self.send_message_to_partner(token, Message::ManageResourceGroup)
                .await;
        }
    }
    fn admin_fence_request(&self, state: &mut HostState) {
        state.admin_requested_fence = true;
        for task in &state.outstanding_resource_tasks {
            task.lost_connection.notify_one();
        }
    }

    fn do_failback(&self, state: &mut HostState, cluster: &Cluster) {
        let still_running = take(&mut state.outstanding_resource_tasks)
            .into_iter()
            .filter(|task| {
                let rg = cluster.get_resource_group(&task.id);

                // If a resource is currently home, there is nothing to do for it.
                if rg.root.home_node.id() == self.id() {
                    return true;
                }

                // If a resource is not home, then need to stop it and pass management on...
                warn!("{} is not home and will be moved back.", &task.id);

                task.switch_host.notify_one();

                false
            })
            .collect();

        state.outstanding_resource_tasks = still_running;
    }

    async fn switch_host(
        &self,
        token: ResourceToken,
        client: &ocf_resource_agent::Client,
        cluster: &Cluster,
    ) -> HostMessage {
        let rg = cluster.get_resource_group(&token.id);

        match rg.stop_resources(client).await {
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

        self.send_message_to_partner(token, Message::ManageResourceGroup)
            .await;

        HostMessage::None
    }

    /// The purpose of this procedure is to perform startup logic to discover the existing state of
    /// resources when the management service starts.
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
    async fn startup(
        &self,
        cluster: &Cluster,
        my_resources: Vec<ResourceToken>,
    ) -> Vec<ResourceToken> {
        let (manage_these, send_these): (Vec<ResourceToken>, Vec<ResourceToken>) =
            match self.get_client().await {
                Ok(client) => {
                    let mut manage_these = Vec::new();
                    let mut send_these = Vec::new();

                    let statuses = my_resources
                        .into_iter()
                        .map(|token| self.home_startup_check(token, cluster, &client));

                    for (token, is_home) in future::join_all(statuses).await {
                        if is_home {
                            manage_these.push(token)
                        } else {
                            send_these.push(token)
                        }
                    }

                    (manage_these, send_these)
                }
                Err(_) => {
                    self.set_connected(false);
                    (Vec::new(), my_resources)
                }
            };

        for token in send_these {
            self.send_message_to_partner(token, Message::CheckResourceGroup)
                .await;
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
        match is_resource_group_running_here(&token, cluster, client, false).await {
            Ok(is_running_here) => (token, is_running_here),
            Err(_) => todo!(),
        }
    }

    /// Checks if the resource group appears to be running on the node with the given client.
    ///
    /// - If yes, this sends a Message::ManageResourceGroup message to self, to direct this Host
    ///   task to begin managing the resource group.
    ///
    /// - If not, then behavior depends on whether this is runnong on the home node or not.
    ///   - Home node: send a CheckResourceGroup message to the failover Host, to see if the
    ///     resource might be running there.
    ///   - Failover node: send a ManageResourceGroup back to the home Host, to direct it to begin
    ///     managing the resource group.
    ///
    /// - If an error was observed, returns a Message::ResourceError to inform the main Host task of
    ///   the situation.
    async fn check_resource_group_managed(
        &self,
        token: ResourceToken,
        cluster: &Cluster,
        client: &ocf_resource_agent::Client,
    ) -> HostMessage {
        match is_resource_group_running_here(&token, cluster, client, true).await {
            Ok(is_running_here) => {
                if is_running_here {
                    self.send_message_to_self(token, Message::ManageResourceGroup)
                        .await;
                } else {
                    match token.location {
                        Location::Away => {
                            self.send_message_to_partner(token, Message::ManageResourceGroup)
                                .await;
                        }
                        Location::Home => {
                            self.send_message_to_partner(token, Message::CheckResourceGroup)
                                .await;
                        }
                    }
                };

                HostMessage::None
            }
            Err(ManagementError::Configuration) => new_message(token, Message::ResourceError),
            Err(ManagementError::Connection) => todo!(),
        }
    }

    /// Perform observation of a resource group when the manager process is in "managed mode", but
    /// the resource group itself is set to "managed = false".
    ///
    /// This checks each Host in a pair in turn, and if the resource becomes managed again, then it
    /// needs to redo the startup checks.
    async fn observe_resource_group_managed_mode(
        &self,
        token: ResourceToken,
        cluster: &Cluster,
        client: &ocf_resource_agent::Client,
    ) -> HostMessage {
        let rg = cluster.get_resource_group(&token.id);

        match is_resource_group_running_here(&token, cluster, client, true).await {
            Ok(is_running_here) => {
                if is_running_here {
                    self.send_message_to_self(token, Message::ManageResourceGroup)
                        .await;
                } else if rg.get_managed() {
                    self.send_message_to_partner(token, Message::CheckResourceGroup)
                        .await;
                } else {
                    tokio::time::sleep(tokio::time::Duration::from_millis(cluster.args.sleep_time))
                        .await;
                    self.send_message_to_partner(token, Message::ObserveResourceGroup)
                        .await;
                }
            }
            Err(ManagementError::Configuration) => todo!(),
            Err(ManagementError::Connection) => todo!(),
        };

        HostMessage::None
    }

    /// Management of a resource group proceeds by calling the management loop method on
    /// ResourceGroup. At the same time, however, this task must be cancellable in case management
    /// should end for any reason, so it also listens for the cancel signal.
    async fn manage_resource_group(
        &self,
        cluster: &Cluster,
        token: ResourceToken,
        client: &ocf_resource_agent::Client,
        revoke: ResourceTaskCancel,
    ) -> HostMessage {
        let rg = cluster.get_resource_group(&token.id);

        if !self.active() {
            warn!("Host {} asked to manage resource group {}, but it is inactive. Requesting partner manage it.", self.id(), token.id);
            return new_message(token, Message::SwitchHost);
        }

        debug!("Host {} is managing resource group {}", self.id(), token.id);

        tokio::select! {
            // Biased because if a task has been cancelled, it should exit ASAP and not bother
            // trying to do any more work.
            biased;

            // Received a cancel notification: exit right away.
            _ = revoke.lost_connection.notified() => {
                new_message(token, Message::TaskCanceled)
            }

            _ = revoke.switch_host.notified() => {
                new_message(token, Message::SwitchHost)
            }

            // If the resource management loop returns, it is either because an error was observed,
            // or because the "managed" flag is set to false and the resource was stopped.
            res = rg.manage_loop(client, token.location) => {
                match res {
                    // Resource was stopped, and it is no longer supposed to be managed.
                    // Enter "Observe" mode, starting with a check on the partner host.
                    Ok(()) => {
                        self.send_message_to_partner(token, Message::ObserveResourceGroup)
                            .await;
                        HostMessage::None
                    }
                    Err(ManagementError::Connection) => {
                        debug!("{}: broken connection while managing {}", self.id(), token.id);
                        new_message(token, Message::RequestFailover)
                    }
                    Err(ManagementError::Configuration) => {
                        debug!("host {} got a configuration error when managing resource group {}", self.id(), token.id);
                        new_message(token, Message::ResourceError)
                    }
                }
            }
        }
    }
}
