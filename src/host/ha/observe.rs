// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

//! Observation of a failover cluster with HA pairs.

use std::mem::take;

use {
    futures::StreamExt,
    log::{debug, trace},
};

use crate::cluster::Cluster;

use super::*;

struct HostState {
    resources_to_observe: Vec<ResourceToken>,
    resources_with_errors: Vec<ResourceToken>,
}

impl HostState {
    fn new() -> Self {
        Self {
            resources_to_observe: Vec::new(),
            resources_with_errors: Vec::new(),
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
                        .await
                }
                Err(_) => {
                    debug!(
                        "Host {} failed to establish connection to its remote agent.",
                        self.id()
                    );

                    self.remote_disconnected_loop_observe(cluster).await
                }
            }
        }
    }

    async fn remote_disconnected_loop_observe(&self, cluster: &Cluster) {
        tokio::select! {
            _ = self.remote_liveness_check(cluster) => {}
            _ = self.handle_messages_remote_disconnected_observe() => {}
        }
    }

    async fn handle_messages_remote_disconnected_observe(&self) {
        loop {
            match self.receive_message().await {
                HostMessage::Command(command) => {
                    todo!("Handle command {command:?} in ha observe mode.")
                }
                HostMessage::Resource(event) => {
                    match event.kind {
                        Message::ManageResourceGroup
                        | Message::RequestFailover
                        | Message::SwitchHost
                        | Message::TaskCanceled => {
                            panic!(
                                "Unexpected to receive a {:?} event in observe mode.",
                                event.kind
                            );
                        }
                        Message::ResourceError => {
                            panic!("Unexpected to receive a resource error message in disconnected mode.");
                        }
                        Message::CheckResourceGroup => {
                            self.send_message_to_partner(
                                event.resource_group,
                                Message::ObserveResourceGroup,
                            )
                            .await;
                        }
                        Message::ObserveResourceGroup => {
                            self.send_message_to_partner(
                                event.resource_group,
                                Message::ObserveResourceGroup,
                            )
                            .await;
                        }
                    }
                }
                HostMessage::None(_) => {}
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
            tasks.push(Box::pin(
                self.check_resource_group(token, cluster, client, false),
            ));
        }

        while let Some(event) = tasks.next().await {
            trace!("Host {} got event: {event:?}", self.id());
            match event {
                HostMessage::Command(command) => {
                    todo!("Handle command {command:?} in ha observe mode.")
                }
                HostMessage::Resource(event) => match event.kind {
                    Message::ManageResourceGroup
                    | Message::RequestFailover
                    | Message::SwitchHost
                    | Message::TaskCanceled => {
                        panic!(
                            "Unexpected to receive a {:?} event in observe mode.",
                            event.kind
                        )
                    }
                    Message::CheckResourceGroup => {
                        tokio::time::sleep(tokio::time::Duration::from_millis(
                            cluster.args.sleep_time,
                        ))
                        .await;
                        tasks.push(Box::pin(self.check_resource_group(
                            event.resource_group,
                            cluster,
                            client,
                            true,
                        )));
                        tasks.push(Box::pin(self.receive_message()));
                    }
                    Message::ObserveResourceGroup => {
                        tokio::time::sleep(tokio::time::Duration::from_millis(
                            cluster.args.sleep_time,
                        ))
                        .await;
                        tasks.push(Box::pin(self.observe_resource_group_ha(
                            event.resource_group,
                            cluster,
                            client,
                        )));
                        tasks.push(Box::pin(self.receive_message()));
                    }
                    Message::ResourceError => {
                        state.resources_with_errors.push(event.resource_group)
                    }
                },
                HostMessage::None(_id) => {}
            }
        }
    }

    async fn check_resource_group(
        &self,
        token: ResourceToken,
        cluster: &Cluster,
        client: &ocf_resource_agent::Client,
        update_status_if_stopped: bool,
    ) -> HostMessage {
        let id = token.id.clone();

        match is_resource_group_running_here(&token, cluster, client, update_status_if_stopped)
            .await
        {
            Ok(is_running_here) => {
                if is_running_here {
                    self.send_message_to_self(token, Message::ObserveResourceGroup)
                        .await;

                    HostMessage::None(id)
                } else {
                    tokio::time::sleep(tokio::time::Duration::from_millis(cluster.args.sleep_time))
                        .await;
                    self.send_message_to_partner(token, Message::CheckResourceGroup)
                        .await;

                    HostMessage::None(id)
                }
            }
            Err(ManagementError::Connection) => {
                debug!(
                    "{}: broken connection while checking {}",
                    self.id(),
                    token.id
                );
                self.send_message_to_partner(token, Message::CheckResourceGroup)
                    .await;
                HostMessage::None(id)
            }
            Err(ManagementError::Configuration) => {
                debug!(
                    "host {} got a configuration error when managing resource group {}",
                    self.id(),
                    token.id
                );
                new_message(token, Message::ResourceError)
            }
        }
    }

    async fn observe_resource_group_ha(
        &self,
        token: ResourceToken,
        cluster: &Cluster,
        client: &ocf_resource_agent::Client,
    ) -> HostMessage {
        let id = token.id.clone();
        let rg = cluster.get_resource_group(&id);
        match rg.observe_loop(client, true, token.location).await {
            // Resource stopped: need to see if it started running on partner.
            Ok(()) => {
                tokio::time::sleep(tokio::time::Duration::from_millis(cluster.args.sleep_time))
                    .await;
                self.send_message_to_partner(token, Message::CheckResourceGroup)
                    .await;

                HostMessage::None(id)
            }
            Err(ManagementError::Connection) => {
                debug!(
                    "{}: broken connection while observing {}",
                    self.id(),
                    token.id
                );
                self.send_message_to_partner(token, Message::CheckResourceGroup)
                    .await;
                HostMessage::None(id)
            }
            Err(ManagementError::Configuration) => {
                debug!(
                    "host {} got a configuration error when managing resource group {}",
                    self.id(),
                    token.id
                );
                new_message(token, Message::ResourceError)
            }
        }
    }
}
