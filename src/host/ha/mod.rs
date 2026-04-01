// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use futures::stream::FuturesUnordered;

use crate::{
    cluster::Cluster,
    resource::{Location, ManagementError},
};

use super::*;

pub mod manage;
pub mod observe;

impl Host {
    fn ha_failover_partner(&self) -> &Arc<Host> {
        self.failover_partner()
            .expect("Host without failover partner in HA routine.")
    }

    /// Sends a message of the given type to self.
    async fn send_message_to_self(&self, token: ResourceToken, message: Message) {
        self.sender.send(new_message(token, message)).await.unwrap();
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
