// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

//! Observe-only management of a cluster without high-availability.

use std::cell::Ref;

use futures::{stream::FuturesUnordered, StreamExt};

use crate::Cluster;

use super::*;

struct Event {}

impl Host {
    pub async fn observe(&self, cluster: &Cluster) {
        let client = ClientWrapper::new(self.address().to_string());

        let mut tasks: FuturesUnordered<_> = FuturesUnordered::new();

        let my_resources = cluster
            .host_home_resource_groups(self)
            .map(|rg| rg.id().to_string());

        for rg in my_resources {
            tasks.push(self.observe_resource_group(rg, client.get().await));
        }

        while let Some(_event) = tasks.next().await {
            todo!()
        }
    }

    async fn observe_resource_group(
        &self,
        _rg: String,
        _client: Ref<'_, ocf_resource_agent::Client>,
    ) -> Event {
        Event {}
    }
}
