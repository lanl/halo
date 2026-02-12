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
    log::warn,
    serde::{Deserialize, Serialize},
};

use crate::{
    cluster::Cluster,
    host::{Host, HostCommand},
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

/// The representation of Cluster state that is communicated back to the admin using the status
/// command.
#[derive(Serialize, Deserialize, Debug)]
pub struct ClusterJson {
    pub resources: Vec<ResourceJson>,
    pub hosts: Vec<HostJson>,
}

/// The representation of Resource state that is communicated back to the admin using the status
/// command.
#[derive(Serialize, Deserialize, Debug)]
pub struct ResourceJson {
    pub id: String,
    pub kind: String,
    pub parameters: HashMap<String, String>,
    pub status: String,
    pub comment: Option<String>,
    pub managed: bool,
}

impl ResourceJson {
    fn build(res: &Resource, managed: bool) -> Self {
        let mut comment = None;

        let status = match *res.status.lock().unwrap() {
            ResourceStatus::Unknown(ref reason) => {
                comment = Some(reason.clone());
                "Unknown"
            }
            ResourceStatus::Error(ref reason) => {
                comment = Some(reason.clone());
                "Error"
            }
            ResourceStatus::Stopped => "Stopped",
            ResourceStatus::RunningOnAway => "Running (Failed Over)",
            ResourceStatus::RunningOnHome => "Running",
        }
        .to_string();

        Self {
            id: res.id.clone(),
            kind: res.kind.clone(),
            parameters: res.parameters.clone(),
            status,
            comment,
            managed,
        }
    }
}

/// The representation of Host state that is communicated back to the admin using the status
/// command.
#[derive(Serialize, Deserialize, Debug)]
pub struct HostJson {
    pub id: String,
    pub active: bool,
    pub connected: bool,
}

impl HostJson {
    fn build(host: &Host) -> Self {
        Self {
            id: host.id(),
            active: host.active(),
            connected: host.connected(),
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

        hosts: cluster.hosts().map(|host| HostJson::build(host)).collect(),
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
            warn!(
                "Resource group {}: setting managed={}",
                rg.id(),
                if payload.managed { "true" } else { "false" }
            );
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
    let Some(host) = cluster.get_host(&host_id) else {
        return Err((StatusCode::NOT_FOUND, ""));
    };

    match payload.command.as_str() {
        "failback" => {
            let Some(partner) = host.failover_partner() else {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "Host does not have a failover partner.",
                ));
            };

            partner.command(HostCommand::Failback).await;
        }
        "activate" => {
            let Some(_) = host.failover_partner() else {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "Host does not have a failover partner. Activate command can only be used in HA cluster.",
                ));
            };

            host.set_active(true);
            host.command(HostCommand::Activate).await;
        }
        "deactivate" => {
            let Some(partner) = host.failover_partner() else {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "Host does not have a failover partner. Deactivate command can only be used in HA cluster.",
                ));
            };

            if !partner.active() {
                return Err((StatusCode::CONFLICT, "Partner host is already deactivated. You cannot deactivate both hosts in a pair."));
            }

            host.set_active(false);
            host.command(HostCommand::Deactivate).await;
        }
        _ => return Err((StatusCode::BAD_REQUEST, "Unsupported command.")),
    }

    Ok(())
}
