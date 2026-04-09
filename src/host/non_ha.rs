// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

//! Observe-only management of a cluster without high-availability.

use std::mem::take;

use {
    futures::{future, stream::FuturesUnordered, StreamExt},
    log::{debug, error, warn},
};

use crate::{
    cluster::Cluster,
    resource::{Location, ManagementError, ResourceStatus},
};

use super::*;

/// Mutable state relevant for non-HA resource management.
struct HostState {
    outstanding_resource_tasks: Vec<ResourceTaskCancel>,

    resources_to_manage: Vec<ResourceToken>,

    /// This holds resources that have errors which prevent the management service from managing
    /// them. This includes errors that generally require admin intervention to resolve, for
    /// example a typo in the config file meaning that resource operations fail with "File not
    /// found".
    resources_with_errors: Vec<ResourceToken>,
}

impl HostState {
    fn new() -> Self {
        Self {
            outstanding_resource_tasks: Vec::new(),
            resources_to_manage: Vec::new(),
            resources_with_errors: Vec::new(),
        }
    }

    fn resource_task_exited(&mut self, id: &str) {
        let still_running = take(&mut self.outstanding_resource_tasks)
            .into_iter()
            .filter(|task| task.id != id)
            .collect();

        self.outstanding_resource_tasks = still_running;
    }
}

impl Host {
    pub async fn observe(&self, cluster: &Cluster) {
        loop {
            let client = self.get_client().await.expect("TODO: handle error here.");

            let futures: Vec<_> = cluster
                .host_home_resource_groups(self)
                .map(|rg| self.observe_resource_group(cluster, rg.id(), &client))
                .collect();

            let _ = future::join_all(futures).await;

            // Once all tasks exited (because an RPC error occurred), just wait a bit and try again:
            tokio::time::sleep(tokio::time::Duration::from_millis(cluster.args.sleep_time)).await;
        }
    }

    pub async fn manage(&self, cluster: &Cluster) {
        let mut state = HostState::new();

        state.resources_to_manage = self.mint_resource_tokens(cluster);

        loop {
            match self.get_client().await {
                Ok(client) => {
                    debug!(
                        "Host {} established connection to its remote agent.",
                        self.id()
                    );
                    self.remote_connected_loop_non_ha(&client, cluster, &mut state)
                        .await;

                    self.maybe_do_reboot().await;
                }
                Err(_) => {
                    debug!(
                        "Host {} failed to establish connection to its remote agent.",
                        self.id()
                    );
                    self.remote_disconnected_loop_non_ha(cluster, &mut state)
                        .await;
                }
            }
        }
    }

    async fn remote_disconnected_loop_non_ha(&self, cluster: &Cluster, state: &mut HostState) {
        tokio::select! {
            _ = self.remote_liveness_check(cluster) => {}
            _ = self.handle_messages_remote_disconnected_non_ha(cluster, state) => {}
        }
    }

    async fn handle_messages_remote_disconnected_non_ha(
        &self,
        _cluster: &Cluster,
        _state: &mut HostState,
    ) {
        loop {
            match self.receive_message().await {
                HostMessage::Resource(message) => {
                    panic!("Unexpected to receive message '{message:?}' in non-HA mode.")
                }
                HostMessage::Command(command) => {
                    warn!("Unexpected to receive command '{command:?}' in non-HA mode.")
                }
                HostMessage::None => {
                    panic!("Unexpected to receive message 'None' in non-HA mode.")
                }
            }
        }
    }

    async fn maybe_do_reboot(&self) {
        // TODO: put logic in here to retry connection, etc...
        self.do_reboot().await;
    }

    async fn do_reboot(&self) {
        self.do_fence_nonblocking(FenceCommand::Off)
            .await
            .expect("fence off failed...");
        match self.do_fence_nonblocking(FenceCommand::On).await {
            Ok(()) => warn!("Turning on host {} succeeded.", self.id()),
            Err(e) => warn!("Turning on host {} failed: {e}", self.id()),
        };

        todo!("Finish reboot logic...");
    }

    async fn remote_connected_loop_non_ha(
        &self,
        client: &ocf_resource_agent::Client,
        cluster: &Cluster,
        state: &mut HostState,
    ) {
        let mut tasks: ManagementTasks = FuturesUnordered::new();

        for token in take(&mut state.resources_to_manage) {
            let revoke = ResourceTaskCancel::new(token.id.clone());
            state.outstanding_resource_tasks.push(revoke.clone());
            tasks.push(Box::pin(
                self.manage_resource_group_non_ha(cluster, token, client, revoke),
            ));
        }

        tasks.push(Box::pin(self.receive_message()));

        while let Some(event) = tasks.next().await {
            debug!("Host {} got event: {event:?}", self.id());
            match event {
                HostMessage::Command(command) => {
                    panic!("Unexpected to receive command '{command:?}' in non-HA mode.")
                }
                HostMessage::Resource(event) => {
                    let id = &event.resource_group.id;
                    state.resource_task_exited(id);
                    match event.kind {
                        Message::RequestFailover => {
                            let rg = cluster.get_resource_group(id);
                            rg.root.set_status_recursive(ResourceStatus::Unknown(
                                "Connection to remote host lost.".to_string(),
                            ));
                            if self.ready_for_reboot(state, event.resource_group) {
                                return;
                            }
                        }
                        Message::ResourceError => {
                            state.resources_with_errors.push(event.resource_group)
                        }
                        other => {
                            panic!("Unexpected to receive message '{other:?}' in non-HA mode.")
                        }
                    };
                }
                HostMessage::None => panic!("Unexpected to receive message 'None' in non-HA mode."),
            }
        }
    }

    fn ready_for_reboot(&self, state: &mut HostState, token: ResourceToken) -> bool {
        state.resources_to_manage.push(token);

        for revoke in &state.outstanding_resource_tasks {
            revoke.lost_connection.notify_one();
        }

        state.outstanding_resource_tasks.is_empty()
    }

    async fn observe_resource_group(
        &self,
        cluster: &Cluster,
        rg: &str,
        client: &ocf_resource_agent::Client,
    ) {
        let rg = cluster.get_resource_group(rg);
        let err = rg
            .observe_loop(client, false, Location::Home)
            .await
            .expect_err("observe_loop() should not exit until an error occurs in this usage.");
        error!("{err:?}");
    }

    async fn manage_resource_group_non_ha(
        &self,
        cluster: &Cluster,
        token: ResourceToken,
        client: &ocf_resource_agent::Client,
        revoke: ResourceTaskCancel,
    ) -> HostMessage {
        let rg = cluster.get_resource_group(&token.id);

        tokio::select! {
            biased;

            _ = revoke.lost_connection.notified() => new_message(token, Message::RequestFailover),

            _ = revoke.switch_host.notified() => panic!("Unexpected to receive switch host notification in non-HA mode."),

            res = rg.manage_loop(client, Location::Home) => {
                match res {
                    Ok(()) => todo!("Unsure how to handle this case yet"),
                    Err(e) => match e {
                        ManagementError::Configuration => new_message(token, Message::ResourceError),
                        ManagementError::Connection => new_message(token, Message::RequestFailover),
                    }
                }
            }
        }
    }
}
