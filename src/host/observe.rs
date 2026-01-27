// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

//! Observe-only management of a cluster without high-availability.

use std::cell::Ref;

use futures::future;

use crate::Cluster;

use super::*;

#[allow(clippy::await_holding_refcell_ref)]
impl Host {
    pub async fn observe(&self, cluster: &Cluster) {
        let client = ClientWrapper::new(self.address().to_string());

        loop {
            {
                // Inner scope needed to ensure that all `client_ref`s are dropped before
                // client.clear() below...
                let client_ref = client.get().await;

                let futures: Vec<_> = cluster
                    .host_home_resource_groups(self)
                    .map(|rg| {
                        self.observe_resource_group(cluster, rg.id(), Ref::clone(&client_ref))
                    })
                    .collect();

                let _ = future::join_all(futures).await;
            }

            // Once all tasks exited (because an RPC error occurred), just wait a bit and try again:
            client.clear();
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
        }
    }

    async fn observe_resource_group(
        &self,
        cluster: &Cluster,
        rg: &str,
        client: Ref<'_, ocf_resource_agent::Client>,
    ) {
        let rg = cluster.get_resource_group(rg);
        let err = rg.observe_loop(&client).await;
        eprintln!("received error: {err:?}");
    }
}
