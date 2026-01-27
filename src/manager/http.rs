// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use std::{collections::HashMap, sync::Arc};

use {
    axum::{routing::get, Json, Router},
    serde::Serialize,
};

use crate::{
    cluster::Cluster,
    resource::{Resource, ResourceStatus},
};

/// Main entrypoint for the command server.
///
/// This listens for commands on a unix socket and acts on them.
pub async fn server_main(listener: tokio::net::UnixListener, cluster: Arc<Cluster>) {
    let server = Router::new().route(
        "/status",
        get({
            let cluster = Arc::clone(&cluster);
            || get_status(cluster)
        }),
    );

    axum::serve(listener, server).await.unwrap();
}

#[derive(Serialize)]
struct ClusterJson {
    resources: Vec<ResourceJson>,
}

#[derive(Serialize)]
struct ResourceJson {
    kind: String,
    parameters: HashMap<String, String>,
    status: String,
    managed: bool,
}

impl ResourceJson {
    fn build(res: &Resource) -> Self {
        let status = match *res.status.lock().unwrap() {
            ResourceStatus::Unknown => "Unknown",
            ResourceStatus::Unrunnable => "Unrunnable",
            ResourceStatus::Stopped => "Stopped",
            ResourceStatus::RunningOnAway => "Running (Failed Over)",
            ResourceStatus::RunningOnHome => "Running",
        }
        .to_string();

        Self {
            kind: res.kind.clone(),
            parameters: res.parameters.clone(),
            status,
            managed: *res.managed.lock().unwrap(),
        }
    }
}

async fn get_status(cluster: Arc<Cluster>) -> Json<ClusterJson> {
    let status = ClusterJson {
        resources: cluster.resources().map(ResourceJson::build).collect(),
    };

    Json(status)
}
