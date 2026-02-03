// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

//! Observe-only management of a cluster without high-availability.

use futures::future;

use crate::Cluster;

use super::*;

#[allow(clippy::await_holding_refcell_ref)]
impl Host {
    pub async fn observe(&self, cluster: &Cluster) {
        loop {
            let client = crate::halo_capnp::get_client(&self.address())
                .await
                .expect("TODO: handle error here.");

            let futures: Vec<_> = cluster
                .host_home_resource_groups(self)
                .map(|rg| self.observe_resource_group(cluster, rg.id(), &client))
                .collect();

            let _ = future::join_all(futures).await;

            // Once all tasks exited (because an RPC error occurred), just wait a bit and try again:
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
        }
    }

    async fn observe_resource_group(
        &self,
        cluster: &Cluster,
        rg: &str,
        client: &ocf_resource_agent::Client,
    ) {
        let rg = cluster.get_resource_group(rg);
        let err = rg.observe_loop(client).await;
        eprintln!("received error: {err:?}");
    }
}
