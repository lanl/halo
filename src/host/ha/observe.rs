// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

//! Observation of a failover cluster with HA pairs.

use std::mem::take;

use {
    futures::StreamExt,
    log::{debug, trace},
};

use crate::{cluster::Cluster, resource::ResourceStatus};

use super::*;

struct HostState {
    resources_to_observe: Vec<ResourceToken>,
    resources_with_errors: Vec<ResourceToken>,
    outstanding_resource_tasks: Vec<ResourceTaskCancel>,
}

impl HostState {
    fn new() -> Self {
        Self {
            resources_to_observe: Vec::new(),
            resources_with_errors: Vec::new(),
            outstanding_resource_tasks: Vec::new(),
        }
    }

    fn resource_task_exited(&mut self, id: &str) {
        let still_running = take(&mut self.outstanding_resource_tasks)
            .into_iter()
            .filter(|task| task.id != id)
            .collect();

        self.outstanding_resource_tasks = still_running;
    }
    fn ready_to_exit(&self) -> bool {
        for revoke in &self.outstanding_resource_tasks {
            revoke.lost_connection.notify_one();
        }

        self.outstanding_resource_tasks.is_empty()
    }
}

impl Host {
    pub async fn observe_ha(&self, cluster: &Cluster) {
        let my_resources = self.mint_resource_tokens(cluster);
        debug!("host {}: resources: {my_resources:?}", self.id());

        let mut state = HostState::new();

        state.resources_to_observe = my_resources;

        loop {
            match self.get_client(cluster).await {
                Ok(client) => {
                    debug!(
                        "Host {} established connection to its remote agent.",
                        self.id()
                    );
                    self.remote_connected_loop_observe(&mut state, cluster, &client)
                        .await;
                }
                Err(_) => {
                    debug!(
                        "Host {} failed to establish connection to its remote agent.",
                        self.id()
                    );

                    for token in take(&mut state.resources_to_observe) {
                        self.send_message_to_partner(token, Message::CheckResourceGroup)
                            .await;
                    }

                    self.remote_disconnected_loop_observe(cluster).await;
                }
            }
        }
    }

    async fn remote_disconnected_loop_observe(&self, cluster: &Cluster) {
        tokio::select! {
            _ = self.remote_liveness_check(cluster) => {}
            _ = self.handle_messages_remote_disconnected_observe(cluster) => {}
        }
    }

    async fn handle_messages_remote_disconnected_observe(&self, cluster: &Cluster) {
        loop {
            match self.receive_message().await {
                HostMessage::Command(command) => {
                    todo!("Handle command {command:?} in ha observe mode.")
                }
                HostMessage::Resource(event) => {
                    match event.kind {
                        Message::RequestFailover | Message::SwitchHost | Message::TaskCanceled => {
                            panic!(
                                "Unexpected to receive a {:?} event in observe mode.",
                                event.kind
                            );
                        }
                        Message::ResourceError => {
                            panic!("Unexpected to receive a resource error message in disconnected mode.");
                        }
                        Message::ManageResourceGroup
                        | Message::CheckResourceGroup
                        | Message::ObserveResourceGroup => {
                            // If we received one of these messages, it means the resource is not
                            // running on the partner. But we don't know if it's running here since
                            // we can't reach the remote, so we have to update the status to
                            // unknown.
                            cluster
                                .get_resource_group(&event.resource_group.id)
                                .root
                                .set_status_recursive(ResourceStatus::Unknown(
                                    "Connection to remote host lost.".to_string(),
                                ));
                            self.send_message_to_partner_delayed(
                                event.resource_group,
                                Message::CheckResourceGroup,
                                cluster.args.sleep_time,
                            )
                            .await;
                        }
                    }
                }
                HostMessage::None(_) => {}
                HostMessage::ExitRequested(_) => {}
            }
        }
    }

    async fn remote_connected_loop_observe(
        &self,
        state: &mut HostState,
        cluster: &Cluster,
        client: &ocf_resource_agent::Client,
    ) {
        // Create a set of tasks related to this host's management duties.
        let mut tasks: ManagementTasks = FuturesUnordered::new();

        tasks.push(Box::pin(self.receive_message()));

        for token in take(&mut state.resources_to_observe) {
            self.launch_observe_task(
                &mut tasks,
                state,
                cluster,
                token,
                client,
                Task::CheckPartnerUnknown,
            );
        }

        while let Some(event) = tasks.next().await {
            trace!("Host {} got event: {event:?}", self.id());
            match event {
                HostMessage::Command(command) => {
                    todo!("Handle command {command:?} in ha observe mode.")
                }
                HostMessage::Resource(event) => match event.kind {
                    Message::RequestFailover | Message::SwitchHost => {
                        panic!(
                            "Unexpected to receive a {:?} event in observe mode.",
                            event.kind
                        )
                    }
                    // Although this is "observe-only" mode, the Manage message is used to indicate
                    // that the resource is known to be stopped on the partner - meaning if it is
                    // stopped here as well, the status should be updated to "stopped" instead of
                    // being left at "unknown".
                    Message::ManageResourceGroup => {
                        self.launch_observe_task(
                            &mut tasks,
                            state,
                            cluster,
                            event.resource_group,
                            client,
                            Task::CheckPartnerKnownStopped,
                        );
                    }
                    Message::CheckResourceGroup => {
                        self.launch_observe_task(
                            &mut tasks,
                            state,
                            cluster,
                            event.resource_group,
                            client,
                            Task::CheckPartnerUnknown,
                        );
                    }
                    Message::ObserveResourceGroup => {
                        self.launch_observe_task(
                            &mut tasks,
                            state,
                            cluster,
                            event.resource_group,
                            client,
                            Task::Observe,
                        );
                    }
                    Message::ResourceError => {
                        let id = &event.resource_group.id;
                        state.resource_task_exited(id);
                        state.resources_with_errors.push(event.resource_group)
                    }
                    Message::TaskCanceled => {
                        let id = &event.resource_group.id;
                        state.resource_task_exited(id);
                        cluster.get_resource_group(id).root.set_status_recursive(
                            ResourceStatus::Unknown("Connection to remote host lost.".to_string()),
                        );
                        state.resources_to_observe.push(event.resource_group);
                        if state.ready_to_exit() {
                            return;
                        }
                    }
                },
                HostMessage::None(id) => state.resource_task_exited(&id),
                HostMessage::ExitRequested(id) => {
                    state.resource_task_exited(&id);
                    if state.ready_to_exit() {
                        return;
                    }
                }
            }
        }
    }

    // Returns (true, _) if a network error occurred, (false, _) otherwise.
    async fn check_resource_group(
        &self,
        token: &ResourceToken,
        cluster: &Cluster,
        client: &ocf_resource_agent::Client,
        update_status_if_stopped: bool,
    ) -> (bool, Message) {
        match is_resource_group_running_here(token, cluster, client, update_status_if_stopped).await
        {
            Ok(is_running_here) => {
                if is_running_here {
                    (false, Message::ObserveResourceGroup)
                } else {
                    tokio::time::sleep(tokio::time::Duration::from_millis(cluster.args.sleep_time))
                        .await;
                    (false, Message::ManageResourceGroup)
                }
            }
            Err(ManagementError::Connection) => {
                debug!(
                    "{}: broken connection while checking {}",
                    self.id(),
                    token.id
                );
                (true, Message::CheckResourceGroup)
            }
            Err(ManagementError::Configuration) => {
                debug!(
                    "host {} got a configuration error when managing resource group {}",
                    self.id(),
                    token.id
                );
                (false, Message::ResourceError)
            }
        }
    }

    // Returns (true, _) if a network error occurred, (false, _) otherwise.
    async fn observe_resource_group_ha(
        &self,
        token: &ResourceToken,
        cluster: &Cluster,
        client: &ocf_resource_agent::Client,
    ) -> (bool, Message) {
        let id = &token.id;
        let rg = cluster.get_resource_group(id);
        match rg.observe_loop(client, true, token.location).await {
            // Resource stopped: need to see if it started running on partner.
            Ok(()) => {
                tokio::time::sleep(tokio::time::Duration::from_millis(cluster.args.sleep_time))
                    .await;
                (false, Message::ManageResourceGroup)
            }
            Err(ManagementError::Connection) => {
                debug!("{}: broken connection while observing {}", self.id(), id);
                (true, Message::CheckResourceGroup)
            }
            Err(ManagementError::Configuration) => {
                debug!(
                    "host {} got a configuration error when managing resource group {}",
                    self.id(),
                    id
                );
                (false, Message::ResourceError)
            }
        }
    }

    fn launch_observe_task<'a>(
        &'a self,
        tasks: &mut ManagementTasks<'a>,
        state: &mut HostState,
        cluster: &'a Cluster,
        token: ResourceToken,
        client: &'a ocf_resource_agent::Client,
        task: Task,
    ) {
        let revoke = ResourceTaskCancel::new(token.id.clone());
        state.outstanding_resource_tasks.push(revoke.clone());
        tasks.push(Box::pin(
            self.run_task_with_cancellation(cluster, client, token, revoke, task),
        ));
        tasks.push(Box::pin(self.receive_message()));
    }

    async fn run_task_with_cancellation(
        &self,
        cluster: &Cluster,
        client: &ocf_resource_agent::Client,
        token: ResourceToken,
        revoke: ResourceTaskCancel,
        task: Task,
    ) -> HostMessage {
        tokio::select! {
            biased;

            _ = revoke.lost_connection.notified() => new_message(token, Message::TaskCanceled),

            _ = revoke.switch_host.notified() => panic!("Unexpected to receive switch_host notification in this context"),

            (network_error_occured, msg) = self.run_task(cluster, client, &token, task) => {
                let id = token.id.clone();
                match msg {
                    Message::ResourceError => return new_message(token, Message::ResourceError),
                    Message::ManageResourceGroup | Message::CheckResourceGroup => {
                        self.send_message_to_partner(token, msg).await;
                    }
                    Message::ObserveResourceGroup => {
                        self.send_message_to_self(token, Message::ObserveResourceGroup).await;
                    }

                    other => panic!("Unexpected message type: {other:?}"),
                };
                if network_error_occured {
                    HostMessage::ExitRequested(id)
                } else {
                    HostMessage::None(id)
                }
            }
        }
    }

    async fn run_task(
        &self,
        cluster: &Cluster,
        client: &ocf_resource_agent::Client,
        token: &ResourceToken,
        task: Task,
    ) -> (bool, Message) {
        match task {
            Task::Observe => self.observe_resource_group_ha(token, cluster, client).await,
            Task::CheckPartnerUnknown => {
                self.check_resource_group(token, cluster, client, false)
                    .await
            }
            Task::CheckPartnerKnownStopped => {
                self.check_resource_group(token, cluster, client, true)
                    .await
            }
        }
    }
}

enum Task {
    Observe,
    CheckPartnerUnknown,
    CheckPartnerKnownStopped,
}
