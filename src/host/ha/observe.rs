// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

//! Observation of a failover cluster with HA pairs.

use log::debug;

use crate::cluster::Cluster;

use super::*;

impl Host {
    pub async fn observe_ha(&self, _cluster: &Cluster) {
        loop {
            match get_client(&self.address()).await {
                Ok(client) => self.remote_connected_loop_observe(&client),
                Err(_) => {
                    debug!(
                        "Host {} failed to establish connection to its remote agent.",
                        self.id()
                    );
                }
            }
        }
    }

    fn remote_connected_loop_observe(&self, _client: &ocf_resource_agent::Client) {
        todo!()
    }
}
