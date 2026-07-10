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
    exit_connected_loop_requested: bool,
    resources_in_transit: Vec<ResourceToken>,
}

impl HostState {
    fn new() -> Self {
        Self {
            resources_to_observe: Vec::new(),
            resources_with_errors: Vec::new(),
            outstanding_resource_tasks: Vec::new(),
            exit_connected_loop_requested: false,
            resources_in_transit: Vec::new(),
        }
    }

    fn resource_task_exited(&mut self, id: &str) {
        let still_running = take(&mut self.outstanding_resource_tasks)
            .into_iter()
            .filter(|task| task.id != id)
            .collect();

        self.outstanding_resource_tasks = still_running;
    }

    fn lost_connection(&mut self, id: &str) {
        self.exit_connected_loop_requested = true;
        self.resource_task_exited(id);
        for revoke in &self.outstanding_resource_tasks {
            revoke.lost_connection.notify_one();
        }
    }

    fn should_exit_connected_loop(&mut self) -> bool {
        if !self.exit_connected_loop_requested {
            return false;
        }

        if self.outstanding_resource_tasks.is_empty() {
            self.exit_connected_loop_requested = false;
            true
        } else {
            false
        }
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

                    for rg in take(&mut state.resources_in_transit) {
                        self.send_message_to_partner(rg, Message::CheckResourceGroup)
                            .await;
                    }
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
                    panic!("Unexpected to receive command {command:?} in ha observe mode.")
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
                            );
                        }
                    }
                }
                HostMessage::TaskDone(_) => {}
                HostMessage::MessageFollows => {
                    panic!("Unexpect to get MessageFollows in this context.")
                }
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
                    panic!("Unexpected to receive command {command:?} in ha observe mode.")
                }
                HostMessage::Resource(event) => {
                    tasks.push(Box::pin(self.receive_message()));
                    let id = &event.resource_group.id;
                    match event.kind {
                        Message::SwitchHost => {
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
                            state.resource_task_exited(id);
                            state.resources_with_errors.push(event.resource_group);
                        }
                        Message::TaskCanceled => {
                            state.resource_task_exited(id);
                            cluster.get_resource_group(id).root.set_status_recursive(
                                ResourceStatus::Unknown(
                                    "Connection to remote host lost.".to_string(),
                                ),
                            );
                            state.resources_in_transit.push(event.resource_group);
                        }
                        Message::RequestFailover => {
                            state.lost_connection(id);
                            state.resources_in_transit.push(event.resource_group);
                        }
                    };
                }
                HostMessage::TaskDone(id) => state.resource_task_exited(&id),
                HostMessage::MessageFollows => {}
            }

            if state.should_exit_connected_loop() {
                return;
            }
        }

        unreachable!(
            "Tasks loop should not exit, a receive message task should always be registered."
        )
    }

    async fn check_resource_group(
        &self,
        token: &ResourceToken,
        cluster: &Cluster,
        client: &ocf_resource_agent::Client,
        update_status_if_stopped: bool,
    ) -> (WhereTo, Message) {
        match is_resource_group_running_here(token, cluster, client, update_status_if_stopped).await
        {
            Ok(is_running_here) => {
                if is_running_here {
                    (WhereTo::Here, Message::ObserveResourceGroup)
                } else {
                    tokio::time::sleep(tokio::time::Duration::from_millis(cluster.args.sleep_time))
                        .await;
                    (WhereTo::Partner, Message::ManageResourceGroup)
                }
            }
            Err(ManagementError::Connection) => {
                debug!(
                    "{}: broken connection while checking {}",
                    self.id(),
                    token.id
                );
                (WhereTo::Here, Message::RequestFailover)
            }
            Err(ManagementError::Configuration) => {
                debug!(
                    "host {} got a configuration error when managing resource group {}",
                    self.id(),
                    token.id
                );
                (WhereTo::Here, Message::ResourceError)
            }
        }
    }

    async fn observe_resource_group_ha(
        &self,
        token: &ResourceToken,
        cluster: &Cluster,
        client: &ocf_resource_agent::Client,
    ) -> (WhereTo, Message) {
        let id = &token.id;
        let rg = cluster.get_resource_group(id);
        match rg.observe_loop(client, true, token.location).await {
            // Resource stopped: need to see if it started running on partner.
            Ok(()) => {
                tokio::time::sleep(tokio::time::Duration::from_millis(cluster.args.sleep_time))
                    .await;
                (WhereTo::Partner, Message::ManageResourceGroup)
            }
            Err(ManagementError::Connection) => {
                debug!("{}: broken connection while observing {}", self.id(), id);
                (WhereTo::Here, Message::RequestFailover)
            }
            Err(ManagementError::Configuration) => {
                debug!(
                    "host {} got a configuration error when managing resource group {}",
                    self.id(),
                    id
                );
                (WhereTo::Here, Message::ResourceError)
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
        if state.exit_connected_loop_requested {
            state.resource_task_exited(&token.id);
            state.resources_in_transit.push(token);
            return;
        }

        let revoke = ResourceTaskCancel::new(token.id.clone());
        state.outstanding_resource_tasks.push(revoke.clone());
        tasks.push(Box::pin(
            self.run_task_with_cancellation(cluster, client, token, revoke, task),
        ));
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

            _ = revoke.lost_connection.notified() => {
                self.send_message_to_self(token, Message::TaskCanceled).await;
                HostMessage::MessageFollows
            }

            _ = revoke.switch_host.notified() => panic!("Unexpected to receive switch_host notification in this context"),

            (whereto, msg) = self.run_task(cluster, client, &token, task) => {
                let id = token.id.clone();
                match whereto {
                    WhereTo::Here => {
                        self.send_message_to_self(token, msg).await;
                        HostMessage::MessageFollows
                    }
                    WhereTo::Partner => {
                        self.send_message_to_partner(token, msg).await;
                        HostMessage::TaskDone(id)
                    }
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
    ) -> (WhereTo, Message) {
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
