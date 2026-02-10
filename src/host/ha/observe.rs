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

impl Host {
    pub async fn observe_ha(&self, cluster: &Cluster) {
        let mut my_resources = self.mint_resource_tokens(cluster);
        debug!("host {}: resources: {my_resources:?}", self.id());

        loop {
            match get_client(&self.address()).await {
                Ok(client) => {
                    debug!(
                        "Host {} established connection to its remote agent.",
                        self.id()
                    );
                    self.remote_connected_loop_observe(take(&mut my_resources), cluster, &client)
                        .await
                }
                Err(_) => {
                    debug!(
                        "Host {} failed to establish connection to its remote agent.",
                        self.id()
                    );

                    todo!()
                }
            }
        }
    }

    async fn remote_connected_loop_observe(
        &self,
        resources_to_check: Vec<ResourceToken>,
        cluster: &Cluster,
        client: &ocf_resource_agent::Client,
    ) {
        // Create a set of tasks related to this host's management duties.
        let mut tasks: ManagementTasks = FuturesUnordered::new();

        tasks.push(Box::pin(self.receive_message()));

        for token in resources_to_check {
            tasks.push(Box::pin(
                self.check_resource_group(token, cluster, client, false),
            ));
        }

        while let Some(event) = tasks.next().await {
            trace!("Host {} got event: {event:?}", self.id());
            match event {
                HostMessage::Command(_) => todo!(),
                HostMessage::Resource(event) => match event.kind {
                    Message::CheckResourceGroup => {
                        tasks.push(Box::pin(self.check_resource_group(
                            event.resource_group,
                            cluster,
                            client,
                            true,
                        )));
                        tasks.push(Box::pin(self.receive_message()));
                    }
                    Message::ObserveResourceGroup => {
                        tasks.push(Box::pin(self.observe_resource_group_ha(
                            event.resource_group,
                            cluster,
                            client,
                        )));
                        tasks.push(Box::pin(self.receive_message()));
                    }
                    Message::ManageResourceGroup | Message::RequestFailover => {
                        panic!(
                            "Unexpected to receive a {:?} event in observe mode.",
                            event.kind
                        )
                    }
                    Message::TaskCanceled => todo!(),
                    Message::SwitchHost => todo!(),
                    Message::ResourceError => todo!(),
                },
                HostMessage::None => {}
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
        match is_resource_group_running_here(&token, cluster, client, update_status_if_stopped)
            .await
        {
            Ok(is_running_here) => {
                if is_running_here {
                    self.send_message_to_self(token, Message::ObserveResourceGroup)
                        .await;

                    HostMessage::None
                } else {
                    tokio::time::sleep(tokio::time::Duration::from_millis(cluster.args.sleep_time))
                        .await;
                    self.send_message_to_partner(token, Message::CheckResourceGroup)
                        .await;

                    HostMessage::None
                }
            }
            Err(ManagementError::Configuration) => todo!(),
            Err(ManagementError::Connection) => todo!(),
        }
    }

    async fn observe_resource_group_ha(
        &self,
        token: ResourceToken,
        cluster: &Cluster,
        client: &ocf_resource_agent::Client,
    ) -> HostMessage {
        let rg = cluster.get_resource_group(&token.id);
        match rg.observe_loop(client, true, token.location).await {
            // Resource stopped: need to see if it started running on partner.
            Ok(()) => {
                tokio::time::sleep(tokio::time::Duration::from_millis(cluster.args.sleep_time))
                    .await;
                self.send_message_to_partner(token, Message::CheckResourceGroup)
                    .await;

                HostMessage::None
            }
            Err(ManagementError::Configuration) => todo!(),
            Err(ManagementError::Connection) => todo!(),
        }
    }
}
