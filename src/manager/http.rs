// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use std::{collections::HashMap, sync::Arc};

use {
    axum::{
        extract::Path,
        http::StatusCode,
        routing::{get, patch, post},
        Json, Router,
    },
    serde::{Deserialize, Serialize},
};

use crate::{
    cluster::Cluster,
    host::HostCommand,
    resource::{Resource, ResourceStatus},
};

/// Main entrypoint for the command server.
///
/// This listens for commands on a unix socket and acts on them.
pub async fn server_main(listener: tokio::net::UnixListener, cluster: Arc<Cluster>) {
    let server = Router::new()
        .route(
            "/status",
            get({
                let cluster = Arc::clone(&cluster);
                || get_status(cluster)
            }),
        )
        .route(
            "/resources/{id}",
            patch({
                let cluster = Arc::clone(&cluster);
                |path, payload| set_managed(path, payload, cluster)
            }),
        )
        .route(
            "/hosts/{id}",
            post({
                let cluster = Arc::clone(&cluster);
                |path, payload| host_post(path, payload, cluster)
            }),
        );

    axum::serve(listener, server).await.unwrap();
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ClusterJson {
    pub resources: Vec<ResourceJson>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ResourceJson {
    pub kind: String,
    pub parameters: HashMap<String, String>,
    pub status: String,
    pub managed: bool,
}

impl ResourceJson {
    fn build(res: &Resource, managed: bool) -> Self {
        let status = match *res.status.lock().unwrap() {
            ResourceStatus::Unknown => "Unknown".to_string(),
            ResourceStatus::Error(ref reason) => format!("Error: ({reason})"),
            ResourceStatus::Stopped => "Stopped".to_string(),
            ResourceStatus::RunningOnAway => "Running (Failed Over)".to_string(),
            ResourceStatus::RunningOnHome => "Running".to_string(),
        };

        Self {
            kind: res.kind.clone(),
            parameters: res.parameters.clone(),
            status,
            managed,
        }
    }
}

async fn get_status(cluster: Arc<Cluster>) -> Json<ClusterJson> {
    let status = ClusterJson {
        resources: cluster
            .resource_groups()
            .flat_map(|rg| {
                let managed = rg.get_managed();
                rg.resources()
                    .map(move |res| ResourceJson::build(res, managed))
            })
            .collect(),
    };

    Json(status)
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SetManagedArgs {
    pub managed: bool,
}

async fn set_managed(
    Path(resource_id): Path<String>,
    Json(payload): Json<SetManagedArgs>,
    cluster: Arc<Cluster>,
) -> Result<(), StatusCode> {
    for rg in cluster.resource_groups() {
        if rg.root.id == resource_id {
            rg.set_managed(payload.managed);
            return Ok(());
        }
    }

    Err(StatusCode::NOT_FOUND)
}

#[derive(Serialize, Deserialize, Debug)]
pub struct HostArgs {
    pub command: String,
}

async fn host_post(
    Path(host_id): Path<String>,
    Json(payload): Json<HostArgs>,
    cluster: Arc<Cluster>,
) -> Result<(), (StatusCode, &'static str)> {
    match payload.command.as_str() {
        "failback" => {
            let Some(host) = cluster.get_host(&host_id) else {
                return Err((StatusCode::NOT_FOUND, ""));
            };

            let Some(partner) = host.failover_partner() else {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "Host does not have a failover partner.",
                ));
            };

            partner.command(HostCommand::Failback).await;

            Ok(())
        }
        _ => Err((StatusCode::BAD_REQUEST, "Unsupported command.")),
    }
}
